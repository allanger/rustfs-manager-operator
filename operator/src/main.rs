mod cli;
mod conditions;
mod config;
mod controllers;
mod rc;

use crate::controllers::{rustfs_custom_user, rustfs_instance};
use crate::rc::check_rc;

use actix_web::{App, HttpRequest, HttpResponse, HttpServer, Responder, get, middleware};
use api::api::v1beta1_rustfs_custom_user::RustFSCustomUser;
use clap::Parser;
use kube::{Client, CustomResourceExt};
use tracing::error;
use tracing_subscriber::EnvFilter;

use self::config::read_config_from_file;
use self::controllers::{rustfs_bucket, rustfs_bucket_user};

use api::api::v1beta1_rustfs_bucket::RustFSBucket;
use api::api::v1beta1_rustfs_bucket_user::RustFSBucketUser;
use api::api::v1beta1_rustfs_instance::RustFSInstance;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_t = 60000)]
    /// The address the metric endpoint binds to.
    metrics_port: u16,
    #[arg(long, default_value_t = 8081)]
    /// The address the probe endpoint binds to.
    health_probe_port: u16,
    #[arg(long, default_value_t = true)]
    /// Enabling this will ensure there is only one active controller manager.
    // DB Operator feature flags
    #[arg(long, default_value_t = false)]
    /// If enabled, DB Operator will run full reconciliation only
    /// when changes are detected
    is_change_check_nabled: bool,
    #[arg(long, default_value = "/srv/config/config.json")]
    /// A path to a config file
    config: String,
    /// Set to true to generate crds
    #[arg(long, default_value_t = false)]
    print_crd: bool,
}

#[get("/health")]
async fn health(_: HttpRequest) -> impl Responder {
    HttpResponse::Ok().json("healthy")
}

fn crdgen() {
    println!(
        "---\n{}",
        serde_yaml::to_string(&RustFSInstance::crd()).unwrap()
    );
    println!(
        "---\n{}",
        serde_yaml::to_string(&RustFSBucket::crd()).unwrap()
    );
    println!(
        "---\n{}",
        serde_yaml::to_string(&RustFSBucketUser::crd()).unwrap()
    );
    println!(
        "---\n{}",
        serde_yaml::to_string(&RustFSCustomUser::crd()).unwrap()
    );
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    if args.print_crd {
        crdgen();
        return Ok(());
    }
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
    if let Err(err) = check_rc() {
        error!("{}", err);
        std::process::exit(1);
    }
    let client = Client::try_default()
        .await
        .expect("failed to create kube Client");
    let config = read_config_from_file(args.config)?;
    let rustfs_instance_ctrl = rustfs_instance::run(client.clone());
    let rustfs_bucket_ctrl = rustfs_bucket::run(client.clone(), config.clone());
    let rustfs_bucket_user_ctrl = rustfs_bucket_user::run(client.clone(), config.clone());
    let rustfs_custom_user_ctrl = rustfs_custom_user::run(client.clone(), config.clone());
    // Start web server
    let server = HttpServer::new(move || {
        App::new()
            .wrap(middleware::Logger::default().exclude("/health"))
            .service(health)
    })
    .bind("0.0.0.0:8080")?
    .shutdown_timeout(5);

    // Both runtimes implements graceful shutdown, so poll until both are done
    tokio::join!(
        rustfs_instance_ctrl,
        rustfs_bucket_ctrl,
        rustfs_bucket_user_ctrl,
        rustfs_custom_user_ctrl,
        server.run()
    )
    .4?;
    Ok(())
}
