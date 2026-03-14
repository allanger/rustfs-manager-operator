use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use kube::{self};
use schemars::JsonSchema;

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub enum Access {
    #[serde(rename = "readOnly")]
    ReadOnly,
    #[serde(rename = "readWrite")]
    ReadWrite,
}

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    kind = "RustFSBucketUser",
    group = "rustfs.badhouseplants.net",
    version = "v1beta1",
    doc = "Manage users on a RustFs instance",
    shortname = "bucketuser",
    namespaced,
    status = "RustFSBucketUserStatus",
    printcolumn = r#"{"name":"User Name","type":"string","description":"The name of the user","jsonPath":".status.username"}"#,
    printcolumn = r#"{"name":"Secret","type":"string","description":"The name of the secret","jsonPath":".status.secretName"}"#,
    printcolumn = r#"{"name":"ConfigMap","type":"string","description":"The name of the configmap","jsonPath":".status.configMapName"}"#,
    printcolumn = r#"{"name":"Access","type":"string","description":"Which access is set to the user","jsonPath":".spec.access"}"#,
    printcolumn = r#"{"name":"Status","type":"boolean","description":"Is the S3Instance ready","jsonPath":".status.ready"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RustFSUserSpec {
    pub bucket: String,
    #[serde(default)]
    pub cleanup: bool,
    pub access: Access,
}

#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RustFSBucketUserStatus {
    #[serde(default)]
    pub ready: bool,
    //#[schemars(schema_with = "conditions")]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub password_hash: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub policy: Option<String>,
    #[serde(default)]
    pub secret_name: Option<String>,
    #[serde(default)]
    pub config_map_name: Option<String>,
}
