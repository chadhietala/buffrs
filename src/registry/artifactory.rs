// (c) Copyright 2023 Helsing GmbH. All rights reserved.

use eyre::{ensure, Context};
use serde::{Deserialize, Serialize};
use url::Url;

use super::Registry;
use crate::{manifest::Dependency, package::Package};

/// The registry implementation for artifactory
pub struct Artifactory(ArtifactoryConfig);

#[async_trait::async_trait]
impl Registry for Artifactory {
    /// Downloads a package from artifactory
    async fn download(&self, dependency: Dependency) -> eyre::Result<Package> {
        let artifact_uri: Url = format!(
            "{}/{}/{}/{}-{}.tgz",
            self.0.url,
            dependency.manifest.repository,
            dependency.package,
            dependency.package,
            dependency.manifest.version
        )
        .parse()
        .wrap_err("Failed to construct artifact uri")?;

        let response = reqwest::Client::new()
            .get(artifact_uri.clone())
            .basic_auth(self.0.username.to_owned(), Some(self.0.password()?))
            .send()
            .await?;

        ensure!(
            response.status().is_success(),
            "Failed to fetch {dependency}"
        );

        tracing::debug!("downloaded dependency {dependency}");

        let tgz = response.bytes().await.wrap_err("Failed to download tar")?;

        Ok(Package::new(
            dependency.package,
            dependency.manifest.version,
            tgz,
        ))
    }

    /// Publishes a package to artifactory
    async fn publish(&self, package: Package, repository: String) -> eyre::Result<()> {
        let artifact_uri: Url = format!(
            "{}/{}/{}/{}-{}.tgz",
            self.0.url, repository, package.name, package.name, package.version
        )
        .parse()
        .wrap_err("Failed to construct artifact uri")?;

        let response = reqwest::Client::new()
            .put(artifact_uri.clone())
            .basic_auth(self.0.username.to_owned(), Some(self.0.password()?))
            .body(package.tgz)
            .send()
            .await?;

        ensure!(
            response.status().is_success(),
            "Failed to publish {}",
            package.name
        );

        tracing::info!(
            "+ pubished {}/{}@{}",
            repository,
            package.name,
            package.version
        );

        Ok(())
    }
}

impl From<ArtifactoryConfig> for Artifactory {
    fn from(cfg: ArtifactoryConfig) -> Self {
        Self(cfg)
    }
}

/// Authentication data and settings for the artifactory registry
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactoryConfig {
    pub url: Url,
    pub username: String,
}

impl ArtifactoryConfig {
    /// Creates a new artifactory config in the system keyring
    pub fn new(url: Url, username: String, password: String) -> eyre::Result<Self> {
        let cfg = Self { url, username };

        cfg.entry()?
            .set_password(&password)
            .wrap_err("Failed to store password in keyring")?;

        Ok(cfg)
    }

    /// Clears the artifactory config from the system keyring
    pub fn clear(self) -> eyre::Result<()> {
        self.entry()?
            .delete_password()
            .wrap_err("Failed to delete password from keyring")?;

        Ok(())
    }

    /// Loads the password for this artifactory config
    fn password(&self) -> eyre::Result<String> {
        self.entry()?
            .get_password()
            .wrap_err("Failed to load password from keyring, please login")
    }

    /// Accesses the keyring entry associated with this artifactory config
    fn entry(&self) -> eyre::Result<keyring::Entry> {
        keyring::Entry::new(self.url.as_str(), &self.username)
            .wrap_err("Failed to load keyring entry")
    }
}
