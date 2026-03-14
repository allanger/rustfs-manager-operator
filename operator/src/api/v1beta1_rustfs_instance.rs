use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use kube::{self};
use schemars::JsonSchema;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    kind = "RustFSInstance",
    group = "rustfs.badhouseplants.net",
    version = "v1beta1",
    shortname = "rustfs",
    doc = "Connect the operator to a RustFs cluster using this resource",
    status = "RustFSInstanceStatus",
    printcolumn = r#"{"name":"Endpoint","type":"string","description":"The URL of the instance","jsonPath":".spec.endpoint"}"#,
    printcolumn = r#"{"name":"Region","type":"string","description":"The region of the instance","jsonPath":".status.region"}"#,
    printcolumn = r#"{"name":"Total Buckets","type":"number","description":"How many buckets are there on the instance","jsonPath":".status.total_buckets"}"#,
    printcolumn = r#"{"name":"Status","type":"boolean","description":"Is the S3Instance ready","jsonPath":".status.ready"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct S3InstanceSpec {
    pub endpoint: String,
    pub credentials_secret: NamespacedName,
}

/// The status object of `DbInstance`
#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
pub struct RustFSInstanceStatus {
    #[serde(default)]
    pub ready: bool,
    //#[schemars(schema_with = "conditions")]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub buckets: Option<Vec<String>>,
    #[serde(default)]
    pub total_buckets: Option<usize>,
    #[serde(default)]
    pub region: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, JsonSchema)]
pub struct NamespacedName {
    #[serde(rename = "namespace")]
    pub namespace: String,
    #[serde(rename = "name")]
    pub name: String,
}
