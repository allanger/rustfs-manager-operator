use crate::conditions::{init_conditions, is_condition_true, is_condition_unknown, set_condition};
use crate::rc::{RcAlias, admin_info, get_aliases, list_buckets, set_alias};
use api::api::v1beta1_rustfs_instance::{RustFSInstance, RustFSInstanceStatus};
use futures::StreamExt;
use k8s_openapi::api::core::v1::Secret;
use kube::api::{ListParams, PostParams};
use kube::runtime::Controller;
use kube::runtime::controller::Action;
use kube::runtime::events::Recorder;
use kube::runtime::watcher::Config;
use kube::{Api, Client, Error, ResourceExt};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::*;

const TYPE_CONNECTED: &str = "RustFSReady";
const TYPE_SECRET_LABELED: &str = "SecretLabeled";
const FIN_SECRET_LABEL: &str = "s3.badhouseplants.net/s3-label";
const SECRET_LABEL: &str = "s3.badhouseplants.net/s3-instance";

pub(crate) const ACCESS_KEY: &str = "ACCESS_KEY";
pub(crate) const SECRET_KEY: &str = "SECRET_KEY";

#[instrument(skip(ctx, req), fields(trace_id, controller = "rustfs-instance"))]
pub(crate) async fn reconcile(
    req: Arc<RustFSInstance>,
    ctx: Arc<Context>,
) -> RustFSInstanceResult<Action> {
    info!("Staring to reconcile");

    info!("Getting the RustFSInstance resource");
    let rustfs_api: Api<RustFSInstance> = Api::all(ctx.client.clone());
    let mut rustfs_cr = match rustfs_api.get(req.name_any().as_str()).await {
        Ok(res) => res,
        Err(Error::Api(ae)) if ae.code == 404 => {
            info!("Object is not found, probably removed");
            return Ok(Action::await_change());
        }
        Err(err) => return Err(RustFSInstanceError::KubeError(err)),
    };

    let secret_ns = rustfs_cr.clone().spec.credentials_secret.namespace;
    let secret_api: Api<Secret> = Api::namespaced(ctx.client.clone(), &secret_ns);

    // If status is none, we need to initialize the object
    let mut status = match rustfs_cr.clone().status {
        None => {
            info!("Status is not yet set, initializing the object");
            return init_object(rustfs_cr, rustfs_api).await;
        }
        Some(status) => status,
    };

    // We need to know the secret before deletion, because the operator needs to unlabel it
    let secret = match get_secret(secret_api.clone(), rustfs_cr.clone()).await {
        Ok(secret) => secret,
        Err(err) => return Err(RustFSInstanceError::KubeError(err)),
    };

    // Handle the deletion logic
    if rustfs_cr.metadata.deletion_timestamp.is_some() {
        info!("Object is marked for deletion");
        if let Some(mut finalizers) = rustfs_cr.clone().metadata.finalizers {
            if finalizers.contains(&FIN_SECRET_LABEL.to_string()) {
                info!("Removing labels from the secret with credentials");
                match unlabel_secret(ctx.clone(), rustfs_cr.clone(), secret).await {
                    Ok(_) => {
                        if let Some(index) = finalizers.iter().position(|x| x == FIN_SECRET_LABEL) {
                            finalizers.remove(index);
                        };
                    }
                    Err(err) => return Err(RustFSInstanceError::KubeError(err)),
                };
            }
            rustfs_cr.metadata.finalizers = Some(finalizers);
        };

        match rustfs_api
            .replace(&rustfs_cr.name_any(), &PostParams::default(), &rustfs_cr)
            .await
        {
            Ok(_) => return Ok(Action::await_change()),
            Err(err) => return Err(RustFSInstanceError::KubeError(err)),
        }
    }

    // If secret is labeled, add a finalizer to the rustfs_cr
    if is_condition_true(status.clone().conditions, TYPE_SECRET_LABELED) {
        let mut current_finalizers = rustfs_cr.clone().metadata.finalizers.unwrap_or_default();
        // Only if the finalizer is not added yet
        if !current_finalizers.contains(&FIN_SECRET_LABEL.to_string()) {
            info!("Adding a finalizer");
            current_finalizers.push(FIN_SECRET_LABEL.to_string());

            rustfs_cr.metadata.finalizers = Some(current_finalizers);
            match rustfs_api
                .replace(&rustfs_cr.name_any(), &PostParams::default(), &rustfs_cr)
                .await
            {
                Ok(_) => return Ok(Action::await_change()),
                Err(err) => return Err(RustFSInstanceError::KubeError(err)),
            }
        }
    }

    // Label the secret, if not yet labeled
    if is_condition_unknown(status.clone().conditions, TYPE_SECRET_LABELED) {
        if is_secret_labeled(secret.clone()) {
            if is_secret_labeled_by_another_obj(rustfs_cr.clone(), secret.clone()) {
                return Err(RustFSInstanceError::SecretIsAlreadyLabeled);
            }

            if is_secret_labeled_by_obj(rustfs_cr.clone(), secret.clone()) {
                info!("Secret is already labeled");
                status.conditions = set_condition(
                    status.clone().conditions,
                    req.metadata.clone(),
                    TYPE_SECRET_LABELED,
                    "True".to_string(),
                    "Reconciled".to_string(),
                    "Secret is already labeled".to_string(),
                );
            }
        } else {
            info!("Labeling the secret");
            if let Err(err) = label_secret(ctx.clone(), rustfs_cr.clone(), secret).await {
                return Err(RustFSInstanceError::KubeError(err));
            };

            status.conditions = set_condition(
                status.clone().conditions,
                req.metadata.clone(),
                TYPE_SECRET_LABELED,
                "True".to_string(),
                "Reconciled".to_string(),
                "Secret is labeled".to_string(),
            );
        };

        rustfs_cr.status = Some(status);
        match rustfs_api
            .replace_status(&rustfs_cr.name_any(), &PostParams::default(), &rustfs_cr)
            .await
        {
            Ok(_) => return Ok(Action::await_change()),
            Err(err) => return Err(RustFSInstanceError::KubeError(err)),
        }
    };

    info!("Checking if the secret is labeled by another object");
    if !is_secret_labeled_by_obj(rustfs_cr.clone(), secret.clone()) {
        status.conditions = set_condition(
            status.conditions,
            rustfs_cr.clone().metadata,
            TYPE_SECRET_LABELED,
            "Unknown".to_string(),
            "RustFSInstanceReconciliation".to_string(),
            "Secret is not labeled".to_string(),
        );
        rustfs_cr.status = Some(status);
        match rustfs_api
            .replace_status(
                &rustfs_cr.clone().name_any(),
                &PostParams::default(),
                &rustfs_cr,
            )
            .await
        {
            Ok(_) => return Ok(Action::await_change()),
            Err(err) => return Err(RustFSInstanceError::KubeError(err)),
        }
    };

    info!("Getting data from the secret");
    let (access_key, secret_key) = match get_data_from_secret(secret) {
        Ok((ak, sk)) => (ak, sk),
        Err(err) => return Err(RustFSInstanceError::InvalidSecret(err)),
    };

    let current_aliases = match get_aliases() {
        Ok(aliases) => aliases,
        Err(err) => return Err(RustFSInstanceError::RcCliError(err)),
    };

    // Check if alias already exists
    if current_aliases.aliases.is_none_or(|a| {
        !a.contains(&RcAlias {
            name: rustfs_cr.name_any().to_string(),
        })
    }) && let Err(err) = set_alias(
        rustfs_cr.name_any(),
        rustfs_cr.clone().spec.endpoint,
        access_key.clone(),
        secret_key.clone(),
    ) {
        return Err(RustFSInstanceError::RcCliError(err));
    }

    let admin_info = match admin_info(rustfs_cr.name_any().to_string()) {
        Ok(ai) => ai,
        Err(err) => return Err(RustFSInstanceError::RcCliError(err)),
    };

    let bucket_list = match list_buckets(rustfs_cr.name_any().to_string()) {
        Ok(bl) => bl,
        Err(err) => return Err(RustFSInstanceError::RcCliError(err)),
    };

    status.ready = true;
    status.total_buckets = admin_info.buckets;
    status.region = admin_info.region;
    status.buckets = Some(
        bucket_list
            .items
            .unwrap()
            .iter()
            .map(|b| b.clone().key.unwrap())
            .collect(),
    );
    rustfs_cr.status = Some(status);

    match rustfs_api
        .replace_status(&rustfs_cr.name_any(), &PostParams::default(), &rustfs_cr)
        .await
    {
        Ok(_) => return Ok(Action::requeue(Duration::from_secs(120))),
        Err(err) => return Err(RustFSInstanceError::KubeError(err)),
    };
}

// Bootstrap the object by adding a default status to it
async fn init_object(
    mut obj: RustFSInstance,
    api: Api<RustFSInstance>,
) -> Result<Action, RustFSInstanceError> {
    let conditions = init_conditions(vec![
        TYPE_CONNECTED.to_string(),
        TYPE_SECRET_LABELED.to_string(),
    ]);
    obj.status = Some(RustFSInstanceStatus {
        conditions,
        ..Default::default()
    });
    match api
        .replace_status(obj.clone().name_any().as_str(), &Default::default(), &obj)
        .await
    {
        Ok(_) => Ok(Action::await_change()),
        Err(err) => Err(RustFSInstanceError::KubeError(err)),
    }
}

// Get the secret with credentials
pub(crate) async fn get_secret(
    api: Api<Secret>,
    obj: RustFSInstance,
) -> Result<Secret, kube::Error> {
    api.get(&obj.spec.credentials_secret.name).await
}

async fn unlabel_secret(
    ctx: Arc<Context>,
    obj: RustFSInstance,
    mut secret: Secret,
) -> Result<(), kube::Error> {
    let secret_ns = obj.clone().spec.credentials_secret.namespace;
    let api: Api<Secret> = Api::namespaced(ctx.client.clone(), &secret_ns);
    if let Some(mut labels) = secret.clone().metadata.labels {
        labels.remove(SECRET_LABEL);
        secret.metadata.labels = Some(labels);
        api.replace(&secret.name_any(), &PostParams::default(), &secret)
            .await?;
    }
    Ok(())
}

async fn label_secret(
    ctx: Arc<Context>,
    obj: RustFSInstance,
    mut secret: Secret,
) -> Result<Secret, kube::Error> {
    let secret_ns = obj.clone().spec.credentials_secret.namespace;
    let api: Api<Secret> = Api::namespaced(ctx.client.clone(), &secret_ns);

    secret
        .clone()
        .metadata
        .labels
        .get_or_insert_with(BTreeMap::new)
        .insert(SECRET_LABEL.to_string(), obj.name_any());

    let mut labels = match &secret.clone().metadata.labels {
        Some(labels) => labels.clone(),
        None => {
            let map: BTreeMap<String, String> = BTreeMap::new();
            map
        }
    };
    labels.insert(SECRET_LABEL.to_string(), obj.name_any());
    secret.metadata.labels = Some(labels);
    api.replace(&secret.name_any(), &PostParams::default(), &secret)
        .await?;

    let secret = match api.get(&obj.spec.credentials_secret.name).await {
        Ok(secret) => secret,
        Err(err) => return Err(err),
    };
    Ok(secret)
}

// Checks whether a secret ia already labeled by the operator
fn is_secret_labeled(secret: Secret) -> bool {
    match secret.metadata.labels {
        Some(labels) => labels.get_key_value(SECRET_LABEL).is_some(),
        None => false,
    }
}

// Checks whether a secret is already labeled by another object
fn is_secret_labeled_by_another_obj(obj: RustFSInstance, secret: Secret) -> bool {
    match secret.metadata.labels {
        Some(labels) => labels
            .get(SECRET_LABEL)
            .is_some_and(|label| label != &obj.name_any()),
        None => false,
    }
}

// Checks whether a secret is already labeled by this object
fn is_secret_labeled_by_obj(obj: RustFSInstance, secret: Secret) -> bool {
    match secret.metadata.labels {
        Some(labels) => labels
            .get(SECRET_LABEL)
            .is_some_and(|label| label == &obj.name_any()),
        None => false,
    }
}

// Returns (access_key, secret_key)
pub(crate) fn get_data_from_secret(secret: Secret) -> Result<(String, String), anyhow::Error> {
    let data = match secret.data {
        Some(data) => data,
        None => return Err(anyhow::Error::msg("empty data")),
    };

    let access_key = match data.get(ACCESS_KEY) {
        Some(access_key) => String::from_utf8(access_key.0.clone()).unwrap(),
        None => return Err(anyhow::Error::msg("empty access key")),
    };

    let secret_key = match data.get(SECRET_KEY) {
        Some(secret_key) => String::from_utf8(secret_key.0.clone()).unwrap(),
        None => return Err(anyhow::Error::msg("empty secret key")),
    };

    Ok((access_key, secret_key))
}

pub(crate) fn error_policy(
    _rustfs_cr: Arc<RustFSInstance>,
    err: &RustFSInstanceError,
    _ctx: Arc<Context>,
) -> Action {
    error!(trace.error = %err, "Error occurred during the reconciliation");
    Action::requeue(Duration::from_secs(5 * 60))
}

#[instrument(skip(client), fields(trace_id))]
pub async fn run(client: Client) {
    let s3instances = Api::<RustFSInstance>::all(client.clone());
    if let Err(err) = s3instances.list(&ListParams::default().limit(1)).await {
        error!("{}", err);
        std::process::exit(1);
    }
    let recorder = Recorder::new(client.clone(), "s3instance-controller".into());
    let context = Context { client, recorder };
    Controller::new(s3instances, Config::default().any_semantic())
        .shutdown_on_signal()
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
}

#[derive(Error, Debug)]
pub enum RustFSInstanceError {
    #[error("Kube Error: {0}")]
    KubeError(#[source] kube::Error),

    #[error("SecretIsAlreadyLabeled")]
    SecretIsAlreadyLabeled,

    #[error("Invalid Secret: {0}")]
    InvalidSecret(#[source] anyhow::Error),

    #[error("Error while executing rc cli: {0}")]
    RcCliError(#[source] anyhow::Error),
}

pub type RustFSInstanceResult<T, E = RustFSInstanceError> = std::result::Result<T, E>;
