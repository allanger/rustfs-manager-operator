use serde::{Deserialize, Serialize};
use std::fs::File;

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct OperatorConfig {
    pub set_owner_reference: bool,
}

pub(crate) fn read_config_from_file(path: String) -> Result<OperatorConfig, anyhow::Error> {
    let file = File::open(path)?;
    let config: OperatorConfig = serde_json::from_reader(file)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use tempfile::NamedTempFile;

    use crate::config::read_config_from_file;

    #[test]
    fn test_read_config() {
        let config_json = r#"{
"setOwnerReference": true
}
"#;
        let mut file = NamedTempFile::new().expect("Can't create a file");
        let path = file.path().to_path_buf();
        writeln!(file, "{}", config_json).expect("Can't write a config file");
        let config = read_config_from_file(path.to_str().expect("Can't get the path").to_string())
            .expect("Can't read the config file");
        assert!(config.set_owner_reference);
    }
}
