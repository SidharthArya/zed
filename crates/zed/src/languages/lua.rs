use std::{any::Any, env::consts, path::PathBuf, sync::Arc};

use anyhow::{anyhow, bail, Result};
use async_compression::futures::bufread::GzipDecoder;
use async_tar::Archive;
use async_trait::async_trait;
use client::http::HttpClient;
use futures::{io::BufReader, StreamExt};
use language::LanguageServerName;
use smol::fs;
use util::{async_iife, ResultExt};

use super::installation::{latest_github_release, GitHubLspBinaryVersion};

#[derive(Copy, Clone)]
pub struct LuaLspAdapter;

#[async_trait]
impl super::LspAdapter for LuaLspAdapter {
    async fn name(&self) -> LanguageServerName {
        LanguageServerName("lua-language-server".into())
    }

    async fn server_args(&self) -> Vec<String> {
        vec![
            "--logpath=~/lua-language-server.log".into(),
            "--loglevel=trace".into(),
        ]
    }

    async fn fetch_latest_server_version(
        &self,
        http: Arc<dyn HttpClient>,
    ) -> Result<Box<dyn 'static + Send + Any>> {
        let release = latest_github_release("LuaLS/lua-language-server", http).await?;
        let version = release.name.clone();
        let platform = match consts::ARCH {
            "x86_64" => "x64",
            "aarch64" => "arm64",
            other => bail!("Running on unsupported platform: {other}"),
        };
        let asset_name = format!("lua-language-server-{version}-darwin-{platform}.tar.gz");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == asset_name)
            .ok_or_else(|| anyhow!("no asset found matching {:?}", asset_name))?;
        let version = GitHubLspBinaryVersion {
            name: release.name.clone(),
            url: asset.browser_download_url.clone(),
        };
        Ok(Box::new(version) as Box<_>)
    }

    async fn fetch_server_binary(
        &self,
        version: Box<dyn 'static + Send + Any>,
        http: Arc<dyn HttpClient>,
        container_dir: PathBuf,
    ) -> Result<PathBuf> {
        let version = version.downcast::<GitHubLspBinaryVersion>().unwrap();

        let binary_path = container_dir.join("bin/lua-language-server");

        if fs::metadata(&binary_path).await.is_err() {
            let mut response = http
                .get(&version.url, Default::default(), true)
                .await
                .map_err(|err| anyhow!("error downloading release: {}", err))?;
            let decompressed_bytes = GzipDecoder::new(BufReader::new(response.body_mut()));
            let archive = Archive::new(decompressed_bytes);
            archive.unpack(container_dir).await?;
        }

        fs::set_permissions(
            &binary_path,
            <fs::Permissions as fs::unix::PermissionsExt>::from_mode(0o755),
        )
        .await?;
        Ok(binary_path)
    }

    async fn cached_server_binary(&self, container_dir: PathBuf) -> Option<PathBuf> {
        async_iife!({
            let mut last_binary_path = None;
            let mut entries = fs::read_dir(&container_dir).await?;
            while let Some(entry) = entries.next().await {
                let entry = entry?;
                if entry.file_type().await?.is_file()
                    && entry
                        .file_name()
                        .to_str()
                        .map_or(false, |name| name == "lua-language-server")
                {
                    last_binary_path = Some(entry.path());
                }
            }

            if let Some(path) = last_binary_path {
                Ok(path)
            } else {
                Err(anyhow!("no cached binary"))
            }
        })
        .await
        .log_err()
    }
}