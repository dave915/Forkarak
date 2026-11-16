//! Language server lifecycle and typed navigation. Never label text matches semantic.
use std::path::{Path, PathBuf};
use anyhow::Result;
use pokarak_core::SourceLocation;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ServerConfig {
    pub language: String,
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug)]
pub enum NavigationKind { Definition, References, Implementation }

#[derive(Clone, Debug)]
pub struct NavigationResult {
    pub locations: Vec<SourceLocation>,
    pub semantic: bool,
    pub status: String,
}

pub struct Navigator;
impl Navigator {
    pub fn new(_root: PathBuf, _configs: Vec<ServerConfig>) -> Self { Self }
    pub fn navigate(&mut self, _path: &Path, _line: u32, _column: u32, _kind: NavigationKind) -> Result<NavigationResult> { anyhow::bail!("No language server is configured") }
    pub fn status(&self) -> String { "분석 서버 대기".into() }
    pub fn shutdown(&mut self) {}
}

pub fn default_configs() -> Vec<ServerConfig> { vec![] }

