// (c) Copyright 2023 Helsing GmbH. All rights reserved.

use buffrs::config::Config;
use buffrs::package::PackageId;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about, long_about)]
#[command(propagate_version = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initializes a buffrs setup
    Init {
        /// Sets up the repository as api package
        #[clap(long)]
        api: Option<PackageId>,
    },

    /// Adds dependencies to a manifest file
    Add {
        /// Dependency to add (Format <repository>/<package>@<version>
        dependency: String,
    },
    /// Removes dependencies from a manifest file
    #[clap(alias = "rm")]
    Remove {
        /// Package to remove from the dependencies
        package: PackageId,
    },

    /// Packages and uploads this api to the registry
    #[clap(alias = "pub")]
    Publish {
        /// Destination repository for the release
        #[clap(long)]
        repository: String,
    },

    /// Installs dependencies
    Install,
    /// Uninstalls dependencies
    Uninstall,

    /// Logs you in for a registry
    Login {
        /// Artifactory url (e.g. https://<domain>/artifactory)
        #[clap(long)]
        url: url::Url,
        /// Artifactory username
        #[clap(long)]
        username: String,
    },
    /// Logs you out from a registry
    Logout,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    color_eyre::install()?;

    tracing_subscriber::fmt()
        .compact()
        .without_time()
        .with_level(false)
        .with_file(false)
        .with_target(false)
        .with_line_number(false)
        .try_init()
        .unwrap();

    let cli = Cli::parse();

    let config = Config::load().await?;

    match cli.command {
        Command::Init { api } => cmd::init(api).await?,
        Command::Add { dependency } => cmd::add(dependency).await?,
        Command::Remove { package } => cmd::remove(package).await?,
        Command::Publish { repository } => cmd::publish(config, repository).await?,
        Command::Install => cmd::install(config).await?,
        Command::Uninstall => cmd::uninstall().await?,
        Command::Login { url, username } => cmd::login(config, url, username).await?,
        Command::Logout => cmd::logout(config).await?,
    }

    Ok(())
}

mod cmd {
    use buffrs::{
        config::Config,
        manifest::{ApiManifest, Dependency, Manifest},
        package::{Package, PackageId, PackageStore},
        registry::{Artifactory, ArtifactoryConfig, Registry},
    };
    use eyre::{ensure, Context, ContextCompat};
    use futures::future::try_join_all;

    /// Initializes the project
    pub async fn init(api: Option<PackageId>) -> eyre::Result<()> {
        let mut manifest = Manifest::default();

        if let Some(name) = api {
            manifest.api = Some(ApiManifest {
                name,
                version: "0.0.1".to_owned(),
                description: None,
            });
        }

        ensure!(
            !Manifest::exists().await?,
            "Cant initialize existing project"
        );

        manifest.write().await
    }

    /// Adds a dependency to this project
    pub async fn add(dependency: String) -> eyre::Result<()> {
        let lower_kebab = |c: char| (c.is_lowercase() && c.is_ascii_alphabetic()) || c == '-';

        let (repository, dependency) = dependency
            .trim()
            .split_once('/')
            .wrap_err("Invalid dependency specification")?;

        ensure!(
            repository.chars().all(lower_kebab),
            "Repositories must be in the format <group>-proto-<stability>"
        );

        ensure!(
            repository.contains("-proto-"),
            "Only proto repositories are allowed"
        );

        let (package, version) = dependency
            .split_once('@')
            .wrap_err("Invalid dependency specification")?;

        let package = package.parse::<PackageId>()?;

        ensure!(
            version
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '-'),
            "Version specifications must be in the format <major>.<minor>.<patch>-<tag>"
        );

        let mut manifest = Manifest::read().await?;

        manifest.dependencies.push(Dependency::new(
            repository.to_owned(),
            package,
            version.to_owned(),
        ));

        manifest.write().await
    }

    /// Removes a dependency from this project
    pub async fn remove(package: PackageId) -> eyre::Result<()> {
        let mut manifest = Manifest::read().await?;

        let dependency = manifest
            .dependencies
            .iter()
            .find(|d| d.package != package)
            .wrap_err(eyre::eyre!(
                "Unable to remove unknown dependency {package:?}"
            ))?
            .to_owned();

        manifest.dependencies.retain(|d| *d != dependency);

        PackageStore::uninstall(&dependency.package).await?;

        manifest.write().await
    }

    /// Publishs the api package to the registry
    pub async fn publish(config: Config, repository: String) -> eyre::Result<()> {
        let artifactory = {
            let Some(artifactory) = config.artifactory else {
                eyre::bail!("Unable to publish package to artifactory, please login using `buffrs login`");
            };

            Artifactory::from(artifactory)
        };

        let package = PackageStore::release().await?;

        artifactory.publish(package, repository).await?;

        Ok(())
    }

    /// Installs dependencies
    pub async fn install(config: Config) -> eyre::Result<()> {
        let artifactory = {
            let Some(artifactory) = config.artifactory else {
                eyre::bail!("Unable to install artifactory dependencies, please login using `buffrs login`");
            };

            Artifactory::from(artifactory)
        };

        let manifest = Manifest::read().await?;

        let mut packages = Vec::with_capacity(manifest.dependencies.len());

        for dep in manifest.dependencies {
            packages.push(artifactory.download(dep));
        }

        let packages: Vec<Package> = try_join_all(packages).await?;

        let mut install = Vec::new();

        for package in packages {
            install.push(PackageStore::install(package));
        }

        try_join_all(install).await?;

        Ok(())
    }

    /// Uninstalls dependencies
    pub async fn uninstall() -> eyre::Result<()> {
        PackageStore::clear().await
    }

    /// Logs you in for a registry
    pub async fn login(mut config: Config, url: url::Url, username: String) -> eyre::Result<()> {
        tracing::info!("Please enter your artifactory token:");

        let mut password = String::new();

        std::io::stdin()
            .read_line(&mut password)
            .wrap_err("Failed to read token")?;

        password = password.trim().to_owned();

        config.artifactory = Some(ArtifactoryConfig::new(url, username, password)?);

        config.write().await
    }

    /// Logs you out from a registry
    pub async fn logout(mut config: Config) -> eyre::Result<()> {
        if let Some(cfg) = config.artifactory {
            cfg.clear()?;
        }

        config.artifactory = None;
        config.write().await
    }
}
