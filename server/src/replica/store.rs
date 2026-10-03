//! Where the replica keeps its copies: an S3-compatible bucket, or a directory
//! (another disk, a network share, or a test's temporary folder). Keys are
//! `/`-separated paths under an optional prefix.

use std::path::{Path, PathBuf};

use bytes::Bytes;

use super::s3::{Object, S3, S3Config};
use crate::error::{Error, Result};

pub enum Store {
    Dir(PathBuf),
    S3 { s3: S3, prefix: String },
}

impl Store {
    pub fn bucket(config: S3Config, prefix: &str) -> Result<Self> {
        let prefix = prefix.trim_matches('/');
        Ok(Self::S3 {
            s3: S3::new(config)?,
            prefix: if prefix.is_empty() { String::new() } else { format!("{prefix}/") },
        })
    }

    pub fn dir(path: &Path) -> Self {
        Self::Dir(path.to_path_buf())
    }

    /// Where it is, for logs (never a secret).
    pub fn describe(&self) -> String {
        match self {
            Self::Dir(dir) => dir.display().to_string(),
            Self::S3 { s3, prefix } => format!("s3://{}/{prefix}", s3.bucket()),
        }
    }

    pub async fn put(&self, key: &str, body: Bytes) -> Result<()> {
        match self {
            Self::Dir(dir) => {
                let path = file(dir, key)?;
                let partial = partial(&path);
                tokio::fs::create_dir_all(path.parent().unwrap_or(dir)).await?;
                tokio::fs::write(&partial, &body).await?;
                tokio::fs::rename(&partial, &path).await?;
                Ok(())
            }
            Self::S3 { s3, prefix } => s3.put(&format!("{prefix}{key}"), body).await,
        }
    }

    /// Copies a file in as it is and returns its size. It must not change meanwhile.
    pub async fn put_file(&self, key: &str, source: &Path) -> Result<u64> {
        match self {
            Self::Dir(dir) => {
                let path = file(dir, key)?;
                let partial = partial(&path);
                tokio::fs::create_dir_all(path.parent().unwrap_or(dir)).await?;
                let size = tokio::fs::copy(source, &partial).await?;
                tokio::fs::rename(&partial, &path).await?;
                Ok(size)
            }
            Self::S3 { s3, prefix } => s3.put_file(&format!("{prefix}{key}"), source).await,
        }
    }

    pub async fn get(&self, key: &str) -> Result<Option<Bytes>> {
        match self {
            Self::Dir(dir) => match tokio::fs::read(file(dir, key)?).await {
                Ok(bytes) => Ok(Some(bytes.into())),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(err) => Err(err.into()),
            },
            Self::S3 { s3, prefix } => s3.get(&format!("{prefix}{key}")).await,
        }
    }

    /// Copies an object out into `dest`. False when there's no such object.
    pub async fn get_to_file(&self, key: &str, dest: &Path) -> Result<bool> {
        match self {
            Self::Dir(dir) => match tokio::fs::copy(file(dir, key)?, dest).await {
                Ok(_) => Ok(true),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(err) => Err(err.into()),
            },
            Self::S3 { s3, prefix } => s3.get_to_file(&format!("{prefix}{key}"), dest).await,
        }
    }

    /// Every object whose key starts with `prefix`, in key order, keys without the store's prefix.
    pub async fn list(&self, prefix: &str) -> Result<Vec<Object>> {
        match self {
            Self::Dir(dir) => {
                let (dir, prefix) = (dir.clone(), prefix.to_string());
                tokio::task::spawn_blocking(move || list_dir(&dir, &prefix))
                    .await
                    .map_err(|err| Error::internal(format!("listing the replica failed: {err}")))?
            }
            Self::S3 { s3, prefix: root } => {
                let mut objects = s3.list(&format!("{root}{prefix}")).await?;
                for object in &mut objects {
                    object.key = object.key.strip_prefix(root.as_str()).unwrap_or(&object.key).to_string();
                }
                Ok(objects)
            }
        }
    }

    /// Deletes an object; one that isn't there is fine.
    pub async fn delete(&self, key: &str) -> Result<()> {
        match self {
            Self::Dir(dir) => {
                let path = file(dir, key)?;
                match tokio::fs::remove_file(&path).await {
                    Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
                    _ => {}
                }
                // Folders are only keys' prefixes, as in a bucket: take away
                // the ones this left empty.
                let mut folder = path.parent();
                while let Some(at) = folder.filter(|at| *at != dir.as_path() && at.starts_with(dir)) {
                    if tokio::fs::remove_dir(at).await.is_err() {
                        break;
                    }
                    folder = at.parent();
                }
                Ok(())
            }
            Self::S3 { s3, prefix } => s3.delete(&format!("{prefix}{key}")).await,
        }
    }
}

/// The file a key names in a directory store. Keys are made by fuwa, but this
/// still keeps them inside it.
fn file(dir: &Path, key: &str) -> Result<PathBuf> {
    let mut path = dir.to_path_buf();
    for part in key.split('/') {
        if part.is_empty() || part == "." || part == ".." || part.contains('\\') || part.ends_with(".partial") {
            return Err(Error::internal(format!("{key:?} isn't a replica key")));
        }
        path.push(part);
    }
    Ok(path)
}

fn partial(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".partial");
    PathBuf::from(name)
}

fn list_dir(root: &Path, prefix: &str) -> Result<Vec<Object>> {
    // Start from the deepest folder the prefix names whole.
    let folder = prefix.rsplit_once('/').map(|(folder, _)| folder).unwrap_or("");
    let mut start = root.to_path_buf();
    for part in folder.split('/').filter(|part| !part.is_empty()) {
        start.push(part);
    }
    let mut objects = Vec::new();
    let mut stack = vec![start];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err.into()),
        };
        for entry in entries {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(root).map(Path::to_path_buf) else { continue };
            let key = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            if key.starts_with(prefix) && !key.ends_with(".partial") {
                objects.push(Object { key, size: entry.metadata()?.len() });
            }
        }
    }
    objects.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_directory_works_like_a_bucket() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::dir(dir.path());
        store.put("servers/a/g1/snapshot", Bytes::from_static(b"one")).await.unwrap();
        store.put("servers/a/g1/00000000-0000000000000000", Bytes::from_static(b"two")).await.unwrap();
        store.put("servers/ab/current", Bytes::from_static(b"g")).await.unwrap();
        store.put("node/current", Bytes::from_static(b"g")).await.unwrap();

        let keys = |objects: Vec<Object>| objects.into_iter().map(|o| o.key).collect::<Vec<_>>();
        assert_eq!(
            keys(store.list("servers/a/").await.unwrap()),
            ["servers/a/g1/00000000-0000000000000000", "servers/a/g1/snapshot"]
        );
        assert_eq!(keys(store.list("servers/a").await.unwrap()).len(), 3);
        assert_eq!(keys(store.list("").await.unwrap()).len(), 4);
        assert!(store.list("nothing/").await.unwrap().is_empty());
        assert_eq!(store.get("node/current").await.unwrap().as_deref(), Some(&b"g"[..]));
        assert_eq!(store.get("node/missing").await.unwrap(), None);
        store.delete("node/current").await.unwrap();
        store.delete("node/current").await.unwrap();
        assert_eq!(store.get("node/current").await.unwrap(), None);
        assert!(store.put("../escape", Bytes::new()).await.is_err());
    }
}
