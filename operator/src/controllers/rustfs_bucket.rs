use crate::conditions::{is_condition_true, set_condition};
use crate::config::OperatorConfig;
use crate::rc::{create_bucket, delete_bucket, list_buckets};
use api::api::v1beta1_rustfs_bucket::{RustFSBucket, RustFSBucketStatus};
use api::api::v1beta1_rustfs_instance::RustFSInstance;
use futures::StreamExt;
use k8s_openapi::api::core::v1::ConfigMap;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use kube::api::{ListParams, ObjectMeta, PostParams};
use kube::runtime::Controller;
use kube::runtime::controller::Action;
use kube::runtime::events::Recorder;
use kube::runtime::watcher::Config;
use kube::{Api, Client, Error, Resource, ResourceExt};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::*;

const TYPE_BUCKET_READY: &str = "BucketReady";
const FIN_CLEANUP: &str = "s3.badhouseplants.net/bucket-cleanup";
const CONFIGMAP_LABEL: &str = "s3.badhouseplants.net/s3-bucket";

const AWS_REGION: &str = "AWS_REGION";
const AWS_ENDPOINT_URL: &str = "AWS_ENDPOINT_URL";
const AWS_BUCKET_NAME: &str = "AWS_BUCKET_NAME";

#[instrument(skip(ctx, obj), fields(trace_id, controller = "rustfs-bucket"))]
pub(crate) async fn reconcile(
    obj: Arc<RustFSBucket>,
    ctx: Arc<Context>,
) -> RustFSBucketResult<Action> {
    info!("Staring to reconcile");

    let bucket_api: Api<RustFSBucket> =
        Api::namespaced(ctx.client.clone(), &obj.namespace().unwrap());
    let cm_api: Api<ConfigMap> = Api::namespaced(ctx.client.clone(), &obj.namespace().unwrap());
    let rustfs_api: Api<RustFSInstance> = Api::all(ctx.client.clone());

    info!("Getting the RustFSBucket resource");
    let mut bucket_cr = match bucket_api.get(&obj.name_any()).await {
        Ok(cr) => cr,
        Err(Error::Api(ae)) if ae.code == 404 => {
            info!("Object is not found, probably removed");
            return Ok(Action::await_change());
        }
        Err(err) => return Err(RustFSBucketError::KubeError(err)),
    };

    // On the first reconciliation status is None
    // it needs to be initialized
    let mut status = match bucket_cr.clone().status {
        None => {
            info!("Status is not yet set, initializing the object");
            return init_object(bucket_cr, bucket_api).await;
        }
        Some(status) => status,
    };

    let configmap_name = format!("{}-bucket-info", bucket_cr.name_any());

    info!("Getting the configmap");
    // Get the cm, if it's already there, we need to validate, or create an empty one
    let mut configmap = match get_configmap(cm_api.clone(), &configmap_name).await {
        Ok(configmap) => configmap,
        Err(Error::Api(ae)) if ae.code == 404 => {
            info!("ConfigMap is not found, creating a new one");
            let cm = ConfigMap {
                metadata: ObjectMeta {
                    name: Some(configmap_name.clone()),
                    namespace: Some(bucket_cr.clone().namespace().unwrap()),
                    ..Default::default()
                },
                ..Default::default()
            };
            match create_configmap(cm_api.clone(), cm).await {
                Ok(cm) => cm,
                Err(err) => return Err(RustFSBucketError::KubeError(err)),
            }
        }
        Err(err) => return Err(RustFSBucketError::KubeError(err)),
    };

    info!("Labeling the configmap");
    configmap = match label_configmap(cm_api.clone(), &bucket_cr.name_any(), configmap).await {
        Ok(configmap) => configmap,
        Err(err) => {
            error!("{}", err);
            return Err(RustFSBucketError::KubeError(err));
        }
    };

    if ctx.config.set_owner_reference {
        info!("Setting owner references to the configmap");
        configmap = match own_configmap(cm_api.clone(), bucket_cr.clone(), configmap).await {
            Ok(configmap) => configmap,
            Err(err) => {
                error!("{}", err);
                return Err(RustFSBucketError::KubeError(err));
            }
        };
    };

    if is_condition_true(status.clone().conditions, TYPE_BUCKET_READY) {
        let mut current_finalizers = match bucket_cr.clone().metadata.finalizers {
            Some(finalizers) => finalizers,
            None => vec![],
        };
        if bucket_cr.spec.cleanup {
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

        bucket_cr.metadata.finalizers = Some(current_finalizers);
        if let Err(err) = bucket_api
            .replace(&bucket_cr.name_any(), &PostParams::default(), &bucket_cr)
            .await
        {
            return Err(RustFSBucketError::KubeError(err));
        }
    };

    info!("Getting the RustFSIntsance");
    let rustfs_cr = match rustfs_api.get(&bucket_cr.spec.instance).await {
        Ok(rustfs_cr) => rustfs_cr,
        Err(err) => {
            error!("{}", err);
            return Err(RustFSBucketError::KubeError(err));
        }
    };

    if rustfs_cr.clone().status.is_none_or(|s| !s.ready) {
        info!("Instance is not ready, waiting");
        return Ok(Action::requeue(Duration::from_secs(120)));
    }

    let bucket_name = format!(
        "{}-{}",
        bucket_cr.namespace().unwrap(),
        bucket_cr.name_any()
    );

    info!("Updating the ConfigMap");
    if let Err(err) = ensure_data_configmap(
        cm_api.clone(),
        configmap.clone(),
        rustfs_cr.clone(),
        &bucket_name,
    )
    .await
    {
        return Err(RustFSBucketError::KubeError(err));
    };

    if bucket_cr.metadata.deletion_timestamp.is_some() {
        info!("Object is marked for deletion");
        if let Some(mut finalizers) = bucket_cr.clone().metadata.finalizers {
            if finalizers.contains(&FIN_CLEANUP.to_string()) {
                match delete_bucket(rustfs_cr.name_any(), bucket_name.clone()) {
                    Ok(_) => {
                        if let Some(index) = finalizers.iter().position(|x| x == FIN_CLEANUP) {
                            finalizers.remove(index);
                        };
                    }
                    Err(err) => return Err(RustFSBucketError::RcCliError(err)),
                }
            }
            bucket_cr.metadata.finalizers = Some(finalizers);
        };
        match bucket_api
            .replace(&bucket_cr.name_any(), &PostParams::default(), &bucket_cr)
            .await
        {
            Ok(_) => return Ok(Action::await_change()),
            Err(err) => return Err(RustFSBucketError::KubeError(err)),
        }
    }

    info!("Getting buckets");

    let bucket_list: Vec<String> = match list_buckets(rustfs_cr.name_any().to_string()) {
        Ok(bl) => bl
            .items
            .unwrap()
            .iter()
            .map(|b| b.clone().key.unwrap())
            .collect(),
        Err(err) => return Err(RustFSBucketError::RcCliError(err)),
    };

    if bucket_list.contains(&bucket_name) {
        info!("Bucket already exists");
    } else {
        if let Err(err) = create_bucket(
            rustfs_cr.name_any(),
            bucket_name.clone(),
            bucket_cr.spec.versioning,
            bucket_cr.spec.object_lock,
        ) {
            return Err(RustFSBucketError::RcCliError(err));
        }
    }
    status.ready = true;
    status.conditions = set_condition(
        status.conditions,
        bucket_cr.metadata.clone(),
        TYPE_BUCKET_READY,
        "True".to_string(),
        "Reconciled".to_string(),
        "Bucket is ready".to_string(),
    );
    status.endpoint = Some(rustfs_cr.clone().spec.endpoint);
    status.region = Some(rustfs_cr.clone().status.unwrap().region.unwrap());
    status.bucket_name = Some(bucket_name.clone());
    status.config_map_name = Some(configmap_name);
    bucket_cr.status = Some(status);

    info!("Updating status of the bucket resource");
    match bucket_api
        .replace_status(&bucket_cr.name_any(), &PostParams::default(), &bucket_cr)
        .await
    {
        Ok(_) => return Ok(Action::requeue(Duration::from_secs(120))),
        Err(err) => return Err(RustFSBucketError::KubeError(err)),
    };
}

// Bootstrap the object by adding a default status to it
async fn init_object(
    mut obj: RustFSBucket,
    api: Api<RustFSBucket>,
) -> Result<Action, RustFSBucketError> {
    let conditions = set_condition(
        vec![],
        obj.metadata.clone(),
        TYPE_BUCKET_READY,
        "Unknown".to_string(),
        "Reconciling".to_string(),
        "Reconciliation started".to_string(),
    );
    obj.status = Some(RustFSBucketStatus {
        conditions,
        ..RustFSBucketStatus::default()
    });
    match api
        .replace_status(obj.clone().name_any().as_str(), &Default::default(), &obj)
        .await
    {
        Ok(_) => Ok(Action::await_change()),
        Err(err) => {
            error!("{}", err);
            Err(RustFSBucketError::KubeError(err))
        }
    }
}

// Get the configmap with the bucket data
async fn get_configmap(api: Api<ConfigMap>, name: &str) -> Result<ConfigMap, kube::Error> {
    info!("Getting a configmap: {}", name);
    match api.get(name).await {
        Ok(cm) => Ok(cm),
        Err(err) => Err(err),
    }
}

// Create ConfigMap
async fn create_configmap(api: Api<ConfigMap>, cm: ConfigMap) -> Result<ConfigMap, kube::Error> {
    match api.create(&PostParams::default(), &cm).await {
        Ok(cm) => get_configmap(api, &cm.name_any()).await,
        Err(err) => Err(err),
    }
}

async fn label_configmap(
    api: Api<ConfigMap>,
    bucket_name: &str,
    mut cm: ConfigMap,
) -> Result<ConfigMap, kube::Error> {
    let mut labels = match &cm.clone().metadata.labels {
        Some(labels) => labels.clone(),
        None => {
            let map: BTreeMap<String, String> = BTreeMap::new();
            map
        }
    };
    labels.insert(CONFIGMAP_LABEL.to_string(), bucket_name.to_string());
    cm.metadata.labels = Some(labels);
    api.replace(&cm.name_any(), &PostParams::default(), &cm)
        .await?;

    let cm = match api.get(&cm.name_any()).await {
        Ok(cm) => cm,
        Err(err) => {
            return Err(err);
        }
    };
    Ok(cm)
}

async fn own_configmap(
    api: Api<ConfigMap>,
    bucket_cr: RustFSBucket,
    mut cm: ConfigMap,
) -> Result<ConfigMap, kube::Error> {
    let mut owner_references = match &cm.clone().metadata.owner_references {
        Some(owner_references) => owner_references.clone(),
        None => {
            let owner_references: Vec<OwnerReference> = vec![];
            owner_references
        }
    };

    if owner_references
        .iter()
        .find(|or| or.uid == bucket_cr.uid().unwrap())
        .is_some()
    {
        return Ok(cm);
    }

    let new_owner_reference = OwnerReference {
        api_version: RustFSBucket::api_version(&()).into(),
        kind: RustFSBucket::kind(&()).into(),
        name: bucket_cr.name_any(),
        uid: bucket_cr.uid().unwrap(),
        ..Default::default()
    };

    owner_references.push(new_owner_reference);
    cm.metadata.owner_references = Some(owner_references);
    api.replace(&cm.name_any(), &PostParams::default(), &cm)
        .await?;

    let cm = match api.get(&cm.name_any()).await {
        Ok(cm) => cm,
        Err(err) => {
            return Err(err);
        }
    };
    Ok(cm)
}

async fn ensure_data_configmap(
    api: Api<ConfigMap>,
    mut cm: ConfigMap,
    rustfs_cr: RustFSInstance,
    bucket_name: &String,
) -> Result<ConfigMap, kube::Error> {
    let mut data = match &cm.clone().data {
        Some(data) => data.clone(),
        None => {
            let map: BTreeMap<String, String> = BTreeMap::new();
            map
        }
    };

    data.insert(
        AWS_REGION.to_string(),
        rustfs_cr.status.unwrap().region.unwrap(),
    );
    data.insert(AWS_ENDPOINT_URL.to_string(), rustfs_cr.spec.endpoint);
    data.insert(AWS_BUCKET_NAME.to_string(), bucket_name.clone());
    cm.data = Some(data);
    api.replace(&cm.name_any(), &PostParams::default(), &cm)
        .await?;

    match api.get(&cm.name_any()).await {
        Ok(cm) => Ok(cm),
        Err(err) => Err(err),
    }
}

pub(crate) fn error_policy(
    _: Arc<RustFSBucket>,
    err: &RustFSBucketError,
    _: Arc<Context>,
) -> Action {
    error!(trace.error = %err, "Error occurred during the reconciliation");
    Action::requeue(Duration::from_secs(5 * 60))
}

#[instrument(skip(client), fields(trace_id))]
pub async fn run(client: Client, config: OperatorConfig) {
    let buckets = Api::<RustFSBucket>::all(client.clone());
    if let Err(err) = buckets.list(&ListParams::default().limit(1)).await {
        error!("{}", err);
        std::process::exit(1);
    }
    let recorder = Recorder::new(client.clone(), "bucket-controller".into());
    let context = Context {
        client,
        recorder,
        config,
    };
    Controller::new(buckets, Config::default().any_semantic())
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
    pub(crate) config: OperatorConfig,
}

#[derive(Error, Debug)]
pub enum RustFSBucketError {
    #[error("Kube Error: {0}")]
    KubeError(#[source] kube::Error),
    #[error("Error while executing rc cli: {0}")]
    RcCliError(#[source] anyhow::Error),
}

pub type RustFSBucketResult<T, E = RustFSBucketError> = std::result::Result<T, E>;
