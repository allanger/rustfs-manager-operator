use handlebars::{Handlebars, RenderError};
use serde::{Deserialize, Serialize};
use serde_json::from_str;
use std::io::Write;
use tempfile::{NamedTempFile, tempfile};
use tracing::info;

use crate::cli;

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcAliasList {
    pub(crate) aliases: Option<Vec<RcAlias>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcAlias {
    pub(crate) name: String,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcAdminInfo {
    pub(crate) buckets: Option<usize>,
    pub(crate) region: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcBucketList {
    pub(crate) items: Option<Vec<RcBucket>>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcBucket {
    pub(crate) key: Option<String>,
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RcUserInfo {
    pub(crate) status: String,
    pub(crate) access_key: String,
}

pub(crate) const POLICY_READ_ONLY: &str = r#"{
  "ID": "",
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "",
      "Effect": "Allow",
      "Action": [
        "s3:GetBucketQuota",
        "s3:GetBucketLocation",
        "s3:GetObject"
      ],
      "Resource": [
        "arn:aws:s3:::{{bucket}}",
        "arn:aws:s3:::{{bucket}}/*"
      ],
      "Condition": {}
    }
  ]
}"#;

pub(crate) const POLICY_READ_WRITE: &str = r#"{
  "ID": "",
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "",
      "Effect": "Allow",
      "Action": [
        "s3:*"
      ],
      "Resource": [
        "arn:aws:s3:::{{bucket}}",
        "arn:aws:s3:::{{bucket}}/*"
      ],
      "Condition": {}
    }
  ]
}"#;

pub(crate) fn get_aliases() -> Result<RcAliasList, anyhow::Error> {
    let output = cli::rc_exec(vec!["alias", "list", "--json"])?;
    let alias_list: RcAliasList = from_str::<RcAliasList>(&output)?;
    Ok(alias_list)
}

pub(crate) fn set_alias(
    name: String,
    endpoint: String,
    username: String,
    password: String,
) -> Result<(), anyhow::Error> {
    cli::rc_exec(vec![
        "alias",
        "set",
        name.as_str(),
        endpoint.as_str(),
        username.as_str(),
        password.as_str(),
    ])?;
    Ok(())
}

pub(crate) fn admin_info(name: String) -> Result<RcAdminInfo, anyhow::Error> {
    let output = cli::rc_exec(vec!["admin", "info", "cluster", name.as_str(), "--json"])?;
    let admin_info: RcAdminInfo = from_str::<RcAdminInfo>(&output)?;
    Ok(admin_info)
}

pub(crate) fn list_buckets(name: String) -> Result<RcBucketList, anyhow::Error> {
    let output = cli::rc_exec(vec!["ls", name.as_str(), "--json"])?;
    let bucket_list = from_str::<RcBucketList>(&output)?;
    Ok(bucket_list)
}

pub(crate) fn create_bucket(
    alias: String,
    bucket_name: String,
    versioning: bool,
    object_lock: bool,
) -> Result<(), anyhow::Error> {
    let path_string = format!("{}/{}", alias, bucket_name);
    let path: &str = &path_string;
    let mut args = vec!["mb", path];

    if versioning {
        args.push("--with-versioning");
    }
    if object_lock {
        args.push("--with-lock");
    }
    cli::rc_exec(args)?;
    Ok(())
}

pub(crate) fn create_user(
    alias: String,
    username: String,
    password: String,
) -> Result<(), anyhow::Error> {
    cli::rc_exec(vec![
        "admin",
        "user",
        "add",
        alias.as_str(),
        username.as_str(),
        password.as_str(),
    ])?;
    Ok(())
}

pub(crate) fn delete_user(alias: String, username: String) -> Result<(), anyhow::Error> {
    cli::rc_exec(vec![
        "admin",
        "user",
        "rm",
        alias.as_str(),
        username.as_str(),
    ])?;
    Ok(())
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
pub(crate) struct RcPolicyData {
    pub(crate) bucket: String,
}

pub(crate) fn render_policy(template: String, data: RcPolicyData) -> Result<String, RenderError> {
    let reg = Handlebars::new();
    reg.render_template(&template, &data)
}

pub(crate) fn create_policy(
    alias: String,
    username: String,
    policy: String,
) -> Result<(), anyhow::Error> {
    let mut file = NamedTempFile::new()?;
    let path = file.path().to_path_buf();
    writeln!(file, "{}", policy)?;
    cli::rc_exec(vec![
        "admin",
        "policy",
        "create",
        alias.as_str(),
        username.as_str(),
        path.to_str().unwrap(),
    ])?;
    Ok(())
}

pub(crate) fn assign_policy(alias: String, username: String) -> Result<(), anyhow::Error> {
    cli::rc_exec(vec![
        "admin",
        "policy",
        "attach",
        alias.as_str(),
        username.as_str(),
        "--user",
        username.as_str(),
        "--json",
    ])?;
    Ok(())
}

pub(crate) fn user_info(alias: String, username: String) -> Result<RcUserInfo, anyhow::Error> {
    let output = cli::rc_exec(vec![
        "admin",
        "user",
        "info",
        alias.as_str(),
        username.as_str(),
        "--json",
    ])?;
    let user_info = from_str::<RcUserInfo>(&output)?;
    Ok(user_info)
}

pub(crate) fn delete_bucket(alias: String, bucket_name: String) -> Result<(), anyhow::Error> {
    let path_string = format!("{}/{}", alias, bucket_name);
    let path: &str = &path_string;
    cli::rc_exec(vec!["rb", path, "--force", "--json"])?;
    Ok(())
}

pub(crate) fn check_rc() -> Result<(), anyhow::Error> {
    cli::rc_exec(vec!["--version"])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::from_str;

    use crate::rc::{RcAlias, RcAliasList, RcBucket, RcBucketList};

    #[test]
    fn test_ser_alias_list() {
        let output = r#"{
  "aliases": [
    {
      "name": "test",
      "endpoint": "https://something",
      "region": "us-east-1",
      "bucket_lookup": "auto"
    },
    {
      "name": "test2",
      "endpoint": "https://something",
      "region": "us-east-1",
      "bucket_lookup": "auto"
    }
  ]
}"#;
        let alias_list: RcAliasList = from_str::<RcAliasList>(&output).unwrap();
        let expected_res = RcAliasList {
            aliases: Some(vec![
                RcAlias {
                    name: "test".to_string(),
                },
                RcAlias {
                    name: "test2".to_string(),
                },
            ]),
        };
        assert_eq!(alias_list, expected_res);
    }
    #[test]
    fn test_ser_bucket_list() {
        let output = r#"{
  "items": [
    {
      "key": "check",
      "last_modified": "2026-03-10T19:24:10Z",
      "is_dir": true
    },
    {
      "key": "default-test",
      "last_modified": "2026-03-11T13:24:26Z",
      "is_dir": true
    },
    {
      "key": "test",
      "last_modified": "2026-03-10T19:24:07Z",
      "is_dir": true
    }
  ],
  "truncated": false
}"#;
        let bucket_list = from_str::<RcBucketList>(&output).unwrap();
        let expected_res = RcBucketList {
            items: Some(vec![
                RcBucket {
                    key: Some("test".to_string()),
                },
                RcBucket {
                    key: Some("test2".to_string()),
                },
            ]),
        };
        assert_eq!(bucket_list, expected_res);
    }
}
