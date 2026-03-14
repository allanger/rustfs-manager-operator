use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use kube::{self};
use schemars::JsonSchema;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    kind = "RustFSBucket",
    group = "rustfs.badhouseplants.net",
    version = "v1beta1",
    shortname = "bucket",
    doc = "Manage buckets on a RustFs instance",
    namespaced,
    status = "RustFSBucketStatus",
    printcolumn = r#"{"name":"Bucket Name","type":"string","description":"The name of the bucket","jsonPath":".status.bucketName"}"#,
    printcolumn = r#"{"name":"Endpoint","type":"string","description":"The URL of the instance","jsonPath":".status.endpoint"}"#,
    printcolumn = r#"{"name":"Region","type":"string","description":"The region of the instance","jsonPath":".status.region"}"#,
    printcolumn = r#"{"name":"Status","type":"boolean","description":"Is the S3Instance ready","jsonPath":".status.ready"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RustFSBucketSpec {
    pub instance: String,
    /// When set to true, the operator will try remove the bucket upon object deletion
    #[serde(default)]
    pub cleanup: bool,
    #[serde(default)]
    #[kube(validation = Rule::new("self == oldSelf").message("field is immutable"))]
    pub object_lock: bool,
    #[serde(default)]
    #[kube(validation = Rule::new("self == oldSelf").message("field is immutable"))]
    pub versioning: bool,
}

/// The status object of `DbInstance`
#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RustFSBucketStatus {
    #[serde(default)]
    pub ready: bool,
    //#[schemars(schema_with = "conditions")]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub bucket_name: Option<String>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub region: Option<String>,
    #[serde(default)]
    pub config_map_name: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct NamespacedName {
    #[serde(rename = "namespace")]
    pub namespace: String,
    #[serde(rename = "name")]
    pub name: String,
}
