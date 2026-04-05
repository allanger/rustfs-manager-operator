use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use k8s_openapi::serde::{Deserialize, Serialize};
use kube::CustomResource;
use kube::{self};
use schemars::JsonSchema;

#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, JsonSchema)]
#[kube(
    kind = "RustFSCustomUser",
    group = "rustfs.badhouseplants.net",
    version = "v1beta1",
    doc = "Manage users on a RustFs instance",
    shortname = "rustfsuser",
    namespaced,
    status = "RustFSCustomUserStatus",
    printcolumn = r#"{"name":"User Name","type":"string","description":"The name of the user","jsonPath":".status.username"}"#,
    printcolumn = r#"{"name":"Secret","type":"string","description":"The name of the secret","jsonPath":".status.secretName"}"#,
    printcolumn = r#"{"name":"Status","type":"boolean","description":"Is the S3Instance ready","jsonPath":".status.ready"}"#
)]
#[serde(rename_all = "camelCase")]
pub struct RustFSCustomSpec {
    #[serde(default)]
    pub cleanup: bool,
    pub policy: String,
    pub instance: String,
}

#[derive(Deserialize, Serialize, Clone, Default, Debug, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RustFSCustomUserStatus {
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
    pub secret_name: Option<String>,
}
