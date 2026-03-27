use crate::conditions::{init_conditions, is_condition_true, is_condition_unknown, set_condition};
use crate::config::OperatorConfig;
use crate::rc::{
    POLICY_READ_ONLY, POLICY_READ_WRITE, RcPolicyData, assign_policy, create_bucket, create_policy,
    create_user, delete_user, list_buckets, render_policy, user_info,
};
use anyhow::{Result, anyhow};
use api::api::v1beta1_rustfs_bucket::RustFSBucket;
use api::api::v1beta1_rustfs_bucket_user::{RustFSBucketUser, RustFSBucketUserStatus};
use api::api::v1beta1_rustfs_instance::RustFSInstance;
use argon2::password_hash::SaltString;
use argon2::password_hash::rand_core::OsRng;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use futures::StreamExt;
use k8s_openapi::ByteString;
use k8s_openapi::api::core::v1::Secret;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{ListParams, ObjectMeta, PostParams};
use kube::runtime::Controller;
use kube::runtime::controller::{Action, ReconcileRequest};
use kube::runtime::events::Recorder;
use kube::runtime::reflector::ObjectRef;
use kube::runtime::watcher::Config;
use kube::{Api, Client, Error, Resource, ResourceExt};
use rand::RngExt;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::*;

const TYPE_USER_READY: &str = "UserReady";
const TYPE_SECRET_READY: &str = "SecretReady";
const FIN_CLEANUP: &str = "s3.badhouseplants.net/bucket-cleanup";
const SECRET_LABEL: &str = "s3.badhouseplants.net/s3-bucket";

const AWS_ACCESS_KEY_ID: &str = "AWS_ACCESS_KEY_ID";
const AWS_SECCRET_ACCESS_KEY: &str = "AWS_SECRET_ACCESS_KEY";

const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ\
                        abcdefghijklmnopqrstuvwxyz\
                        0123456789)(*&^%$#@!~";
const PASSWORD_LEN: usize = 40;

#[instrument(skip(ctx, obj), fields(trace_id))]
pub(crate) async fn reconcile(
    obj: Arc<RustFSBucketUser>,
    ctx: Arc<Context>,
) -> RustFSBucketUserResult<Action> {
    info!("Staring reconciling");
    let user_api: Api<RustFSBucketUser> =
        Api::namespaced(ctx.client.clone(), &obj.namespace().unwrap());
    let bucket_api: Api<RustFSBucket> =
        Api::namespaced(ctx.client.clone(), &obj.namespace().unwrap());
    let secret_api: Api<Secret> = Api::namespaced(ctx.client.clone(), &obj.namespace().unwrap());
    let rustfs_api: Api<RustFSInstance> = Api::all(ctx.client.clone());

    info!("Getting the RustFSBucketUser resource");
    let mut user_cr = match user_api.get(&obj.name_any()).await {
        Ok(cr) => cr,
        Err(Error::Api(ae)) if ae.code == 404 => {
            info!("Object is not found, probably removed");
            return Ok(Action::await_change());
        }
        Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
    };

    // On the first reconciliation status is None
    // it needs to be initialized
    let mut status = match user_cr.clone().status {
        None => {
            info!("Status is not yet set, initializing the object");
            return init_object(user_cr, user_api).await;
        }
        Some(status) => status,
    };

    let secret_name = format!("{}-bucket-creds", user_cr.name_any());

    info!("Getting the secret");
    // Get the secret, if it's already there, we need to validate, or create an empty one
    let mut secret = match get_secret(secret_api.clone(), &secret_name).await {
        Ok(secret) => secret,
        Err(Error::Api(ae)) if ae.code == 404 => {
            info!("Secret is not found, creating a new one");
            let secret = Secret {
                metadata: ObjectMeta {
                    name: Some(secret_name.clone()),
                    namespace: Some(user_cr.clone().namespace().unwrap()),
                    ..Default::default()
                },
                ..Default::default()
            };
            match create_secret(secret_api.clone(), secret).await {
                Ok(secret) => secret,
                Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
            }
        }
        Err(err) => {
            error!("{}", err);
            return Err(RustFSBucketUserError::KubeError(err));
        }
    };

    info!("Labeling the secret");
    secret = match label_secret(secret_api.clone(), &user_cr.name_any(), secret).await {
        Ok(secret) => secret,
        Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
    };

    info!("Setting owner references to the secret");
    if ctx.config.set_owner_reference {
        secret = match own_secret(secret_api.clone(), user_cr.clone(), secret).await {
            Ok(secret) => secret,
            Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
        };
    };
    if is_condition_true(status.clone().conditions, TYPE_USER_READY) {
        let mut current_finalizers = match user_cr.clone().metadata.finalizers {
            Some(finalizers) => finalizers,
            None => vec![],
        };
        if user_cr.spec.cleanup {
            if !current_finalizers.contains(&FIN_CLEANUP.to_string()) {
                info!("Adding a finalizer");
                current_finalizers.push(FIN_CLEANUP.to_string());
            }
        } else {
            if current_finalizers.contains(&FIN_CLEANUP.to_string()) {
                if let Some(index) = current_finalizers
                    .iter()
                    .position(|x| *x == FIN_CLEANUP.to_string())
                {
                    current_finalizers.remove(index);
                };
            }
        };
        user_cr.metadata.finalizers = Some(current_finalizers);
        if let Err(err) = user_api
            .replace(&user_cr.name_any(), &PostParams::default(), &user_cr)
            .await
        {
            return Err(RustFSBucketUserError::KubeError(err));
        }
    };

    info!("Getting the RustFsBucket");
    let bucket_cr = match bucket_api.get(&user_cr.spec.bucket).await {
        Ok(bucket) => bucket,
        Err(err) => {
            error!("{}", err);
            return Err(RustFSBucketUserError::KubeError(err));
        }
    };

    let bucket_status = match bucket_cr.clone().status {
        Some(status) => {
            if !status.ready {
                return Err(RustFSBucketUserError::BucketNotReadyError);
            };
            status
        }
        None => {
            return Err(RustFSBucketUserError::BucketNotReadyError);
        }
    };

    info!("Getting the RustFSIntsance");
    let rustfs_cr = match rustfs_api.get(&bucket_cr.spec.instance).await {
        Ok(rustfs_cr) => rustfs_cr,
        Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
    };

    if rustfs_cr.clone().status.is_none_or(|s| !s.ready) {
        info!("Instance is not ready, waiting");
        return Ok(Action::requeue(Duration::from_secs(120)));
    }
    // Check the secret
    let username = format!("{}-{}", user_cr.namespace().unwrap(), user_cr.name_any());

    // If password missing, regen the secret
    // Update the user
    if user_cr.metadata.deletion_timestamp.is_some() {
        info!("Object is marked for deletion");
        if let Some(mut finalizers) = user_cr.clone().metadata.finalizers {
            if finalizers.contains(&FIN_CLEANUP.to_string()) {
                match delete_user(rustfs_cr.name_any(), username) {
                    Ok(_) => {
                        if let Some(index) = finalizers
                            .iter()
                            .position(|x| *x == FIN_CLEANUP.to_string())
                        {
                            finalizers.remove(index);
                        };
                    }
                    Err(err) => return Err(RustFSBucketUserError::RcCliError(err)),
                }
            }
            user_cr.metadata.finalizers = Some(finalizers);
        };
        match user_api
            .replace(&user_cr.name_any(), &PostParams::default(), &user_cr)
            .await
        {
            Ok(_) => return Ok(Action::await_change()),
            Err(err) => return Err(RustFSBucketUserError::KubeError(err)),
        }
    }

    // If secret is not ready, generate a new one
    if !is_condition_true(status.clone().conditions, TYPE_SECRET_READY) {
        let password = generate_password();
        let argon2 = Argon2::default();
        let salt = SaltString::generate(&mut OsRng);
        let password_hash = match argon2.hash_password(&password.as_bytes(), &salt) {
            Ok(hash) => hash.to_string(),
            Err(err) => {
                error!("{}", err);
                return Err(RustFSBucketUserError::IllegalRustFSBucketUser);
            }
        };
        status.password_hash = Some(password_hash);
        match set_secret_data(
            secret_api.clone(),
            secret.clone(),
            username.clone(),
            password.clone(),
        )
        .await
        {
            Ok(_) => {
                status.conditions = set_condition(
                    status.clone().conditions,
                    user_cr.clone().metadata,
                    TYPE_SECRET_READY,
                    "True".to_string(),
                    "Reconciled".to_string(),
                    "Secret is up-to-date".to_string(),
                );
                user_cr.status = Some(status);
                match user_api
                    .replace_status(&user_cr.name_any(), &PostParams::default(), &user_cr)
                    .await
                {
                    Ok(_) => {
                        return Ok(Action::await_change());
                    }
                    Err(err) => {
                        error!("{}", err);
                        return Err(RustFSBucketUserError::KubeError(err));
                    }
                }
            }
            Err(err) => {
                error!("{}", err);
                return Err(RustFSBucketUserError::KubeError(err));
            }
        }
    }

    let credentials = match secret.data {
        Some(data) => match check_credentials(data, username, status.clone().password_hash) {
            Some(creds) => creds,
            None => {
                status.conditions = set_condition(
                    status.clone().conditions,
                    user_cr.clone().metadata,
                    TYPE_SECRET_READY,
                    "False".to_string(),
                    "Reconciled".to_string(),
                    "Invalid credentials in the secret".to_string(),
                );
                user_cr.status = Some(status);
                info!("I'm setting the condition");
                match user_api
                    .replace_status(&user_cr.name_any(), &PostParams::default(), &user_cr)
                    .await
                {
                    Ok(_) => {
                        return Ok(Action::await_change());
                    }
                    Err(err) => {
                        error!("{}", err);
                        return Err(RustFSBucketUserError::KubeError(err));
                    }
                }
            }
        },
        None => {
            status.clone().conditions = set_condition(
                status.clone().conditions,
                user_cr.clone().metadata,
                TYPE_SECRET_READY,
                "False".to_string(),
                "Reconciled".to_string(),
                "Invalid credentials in the secret".to_string(),
            );
            user_cr.status = Some(status);
            match user_api
                .replace_status(&user_cr.name_any(), &PostParams::default(), &user_cr)
                .await
            {
                Ok(_) => {
                    return Ok(Action::await_change());
                }
                Err(err) => {
                    error!("{}", err);
                    return Err(RustFSBucketUserError::KubeError(err));
                }
            }
        }
    };

    let username = credentials.0;
    let password = credentials.1;

    if let Err(err) = create_user(rustfs_cr.name_any(), username.clone(), password) {
        error!("{}", err);
        return Err(RustFSBucketUserError::IllegalRustFSBucketUser);
    }

    let userinfo = match user_info(rustfs_cr.name_any(), username.clone()) {
        Ok(info) => info,
        Err(err) => return Err(RustFSBucketUserError::RcCliError(err)),
    };

    let policy_template = match user_cr.spec.access {
        api::api::v1beta1_rustfs_bucket_user::Access::ReadOnly => POLICY_READ_ONLY,
        api::api::v1beta1_rustfs_bucket_user::Access::ReadWrite => POLICY_READ_WRITE,
    };

    let bucket_name = match bucket_status.bucket_name {
        Some(name) => name,
        None => {
            error!("bucket name is not yet set");
            return Err(RustFSBucketUserError::IllegalRustFSBucketUser);
        }
    };

    let data = RcPolicyData {
        bucket: bucket_name,
    };
    let policy = match render_policy(policy_template.to_string(), data) {
        Ok(policy) => policy,
        Err(err) => return Err(RustFSBucketUserError::RcCliError(anyhow!(err))),
    };
    if let Err(err) = create_policy(rustfs_cr.name_any(), username.clone(), policy.to_string()) {
        return Err(RustFSBucketUserError::RcCliError(err));
    };

    if let Err(err) = assign_policy(rustfs_cr.name_any(), username.clone()) {
        return Err(RustFSBucketUserError::RcCliError(err));
    };

    // create a user
    status.policy = Some(policy);
    status.username = Some(userinfo.access_key);
    status.status = Some(userinfo.status);
    status.ready = true;
    status.secret_name = Some(secret_name);
    status.config_map_name = bucket_status.config_map_name;

    status.conditions = set_condition(
        status.clone().conditions,
        user_cr.metadata.clone(),
        TYPE_USER_READY,
        "True".to_string(),
        "Reconciled".to_string(),
        "User is ready".to_string(),
    );
    user_cr.status = Some(status);

    info!("Updating status of the bucket resource");
    match user_api
        .replace_status(&user_cr.name_any(), &PostParams::default(), &user_cr)
        .await
    {
        Ok(_) => {
            return Ok(Action::requeue(Duration::from_secs(120)));
        }
        Err(err) => {
            error!("{}", err);
            return Err(RustFSBucketUserError::KubeError(err));
        }
    };
}

// Bootstrap the object by adding a default status to it
async fn init_object(
    mut obj: RustFSBucketUser,
    api: Api<RustFSBucketUser>,
) -> Result<Action, RustFSBucketUserError> {
    let conditions = init_conditions(vec![
        TYPE_SECRET_READY.to_string(),
        TYPE_USER_READY.to_string(),
    ]);
    obj.status = Some(RustFSBucketUserStatus {
        conditions,
        ..RustFSBucketUserStatus::default()
    });
    match api
        .replace_status(obj.clone().name_any().as_str(), &Default::default(), &obj)
        .await
    {
        Ok(_) => Ok(Action::await_change()),
        Err(err) => {
            error!("{}", err);
            Err(RustFSBucketUserError::KubeError(err))
        }
    }
}

// Get the secret with the bucket data
async fn get_secret(api: Api<Secret>, name: &str) -> Result<Secret, kube::Error> {
    info!("Getting a secret: {}", name);
    api.get(name).await
}

// checks if the secret has all the required data
fn check_secret_data(secret: Secret) -> bool {
    let data = match secret.data {
        Some(data) => data,
        None => {
            return false;
        }
    };
    data.contains_key(AWS_SECCRET_ACCESS_KEY) && data.contains_key(AWS_ACCESS_KEY_ID)
}

// Returns false if password is not valid
fn check_credentials(
    data: BTreeMap<String, ByteString>,
    username: String,
    password_hash: Option<String>,
) -> Option<(String, String)> {
    let current_username = match data.get(AWS_ACCESS_KEY_ID) {
        Some(username) => String::from_utf8(username.0.clone()).unwrap(),
        None => {
            return None;
        }
    };
    info!("Username is there");

    let current_password = match data.get(AWS_SECCRET_ACCESS_KEY) {
        Some(password) => String::from_utf8(password.0.clone()).unwrap(),
        None => {
            return None;
        }
    };
    info!("Password is there");

    if current_username != username {
        return None;
    };
    info!("hash is {:?}", password_hash);
    info!("Username is fine");
    if let Some(password_hash) = password_hash {
        let parsed_hash = match PasswordHash::new(&password_hash) {
            Ok(hash) => hash,
            Err(_) => {
                return None;
            }
        };

        match Argon2::default().verify_password(current_password.as_bytes(), &parsed_hash) {
            Ok(_) => {
                return Some((current_username, current_password));
            }
            Err(err) => {
                error!("{}", err);
                return None;
            }
        };
    };
    return None;
}

// Create Secret
async fn create_secret(api: Api<Secret>, secret: Secret) -> Result<Secret, kube::Error> {
    match api.create(&PostParams::default(), &secret).await {
        Ok(secret) => get_secret(api, &secret.name_any()).await,
        Err(err) => Err(err),
    }
}

async fn label_secret(
    api: Api<Secret>,
    bucket_name: &str,
    mut secret: Secret,
) -> Result<Secret, kube::Error> {
    let mut labels = match &secret.clone().metadata.labels {
        Some(labels) => labels.clone(),
        None => {
            let map: BTreeMap<String, String> = BTreeMap::new();
            map
        }
    };
    labels.insert(SECRET_LABEL.to_string(), bucket_name.to_string());
    secret.metadata.labels = Some(labels);
    api.replace(&secret.name_any(), &PostParams::default(), &secret)
        .await?;

    let secret = match api.get(&secret.name_any()).await {
        Ok(secret) => secret,
        Err(err) => {
            return Err(err);
        }
    };
    Ok(secret)
}

async fn own_secret(
    api: Api<Secret>,
    user_cr: RustFSBucketUser,
    mut secret: Secret,
) -> Result<Secret, kube::Error> {
    let mut owner_references = match &secret.clone().metadata.owner_references {
        Some(owner_references) => owner_references.clone(),
        None => {
            let owner_references: Vec<OwnerReference> = vec![];
            owner_references
        }
    };

    if owner_references
        .iter()
        .find(|or| or.uid == user_cr.uid().unwrap())
        .is_some()
    {
        return Ok(secret);
    }

    let new_owner_reference = OwnerReference {
        api_version: RustFSBucketUser::api_version(&()).into(),
        kind: RustFSBucketUser::kind(&()).into(),
        name: user_cr.name_any(),
        uid: user_cr.uid().unwrap(),
        ..Default::default()
    };

    owner_references.push(new_owner_reference);
    secret.metadata.owner_references = Some(owner_references);
    api.replace(&secret.name_any(), &PostParams::default(), &secret)
        .await?;

    let secret = match api.get(&secret.name_any()).await {
        Ok(secret) => secret,
        Err(err) => {
            return Err(err);
        }
    };
    Ok(secret)
}

async fn set_secret_data(
    api: Api<Secret>,
    mut secret: Secret,
    username: String,
    password: String,
) -> Result<Secret, kube::Error> {
    let mut data = match &secret.clone().data {
        Some(data) => data.clone(),
        None => {
            let map: BTreeMap<String, ByteString> = BTreeMap::new();
            map
        }
    };

    data.insert(
        AWS_ACCESS_KEY_ID.to_string(),
        ByteString(username.as_bytes().to_vec()),
    );
    data.insert(
        AWS_SECCRET_ACCESS_KEY.to_string(),
        ByteString(password.as_bytes().to_vec()),
    );

    secret.data = Some(data);
    api.replace(&secret.name_any(), &PostParams::default(), &secret)
        .await?;

    match api.get(&secret.name_any()).await {
        Ok(secret) => Ok(secret),
        Err(err) => Err(err),
    }
}

fn generate_password() -> String {
    let mut rng = rand::rng();
    let password: String = (0..PASSWORD_LEN)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            char::from(CHARSET[idx])
        })
        .collect();
    password
}

pub(crate) fn error_policy(
    _: Arc<RustFSBucketUser>,
    err: &RustFSBucketUserError,
    _: Arc<Context>,
) -> Action {
    error!(trace.error = %err, "Error occurred during the reconciliation");
    Action::requeue(Duration::from_secs(5 * 60))
}

fn cr_ref_from_secret(secret: Secret) -> Option<ObjectRef<RustFSBucketUser>> {
    let ns = match secret.namespace() {
        Some(res) => res,
        None => return None,
    };
    match secret.labels().get(SECRET_LABEL) {
        Some(val) => Some(ObjectRef::new(val).within(&ns)),
        None => None,
    }
}

#[instrument(skip(client), fields(trace_id))]
pub async fn run(client: Client, config: OperatorConfig) {
    let users = Api::<RustFSBucketUser>::all(client.clone());
    if let Err(err) = users.list(&ListParams::default().limit(1)).await {
        error!("{}", err);
        std::process::exit(1);
    }
    let recorder = Recorder::new(client.clone(), "user-controller".into());
    let secret_api: Api<Secret> = Api::all(client.clone());
    let context = Context {
        client,
        recorder,
        config,
    };

    Controller::new(users, Config::default().any_semantic())
        .shutdown_on_signal()
        .watches(secret_api, Config::default(), cr_ref_from_secret)
        .run(reconcile, error_policy, Arc::new(context))
        .filter_map(|x| async move { std::result::Result::ok(x) })
        .for_each(|_| futures::future::ready(()))
        .await;
}
// Context for our reconciler
#[derive(Clone)]
pub(crate) struct Context {
    /// Kubernetes client
    pub client: Client,
    /// Event recorder
    pub recorder: Recorder,
    pub(crate) config: OperatorConfig,
}

#[derive(Error, Debug)]
pub enum RustFSBucketUserError {
    #[error("SerializationError: {0}")]
    SerializationError(#[source] serde_json::Error),

    #[error("Kube Error: {0}")]
    KubeError(#[source] kube::Error),

    #[error("Finalizer Error: {0}")]
    // NB: awkward type because finalizer::Error embeds the reconciler error (which is this)
    // so boxing this error to break cycles
    FinalizerError(#[source] Box<kube::runtime::finalizer::Error<RustFSBucketUserError>>),

    #[error("IllegalRustFSBucketUser")]
    IllegalRustFSBucketUser,

    #[error("SecretIsAlreadyLabeled")]
    SecretIsAlreadyLabeled,

    #[error("Invalid Secret: {0}")]
    InvalidSecret(#[source] anyhow::Error),

    #[error("Error while executing rc cli: {0}")]
    RcCliError(#[source] anyhow::Error),
    #[error("Bucket is not yet ready")]
    BucketNotReadyError,
}

pub type RustFSBucketUserResult<T, E = RustFSBucketUserError> = std::result::Result<T, E>;
