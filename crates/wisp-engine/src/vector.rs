//! Vector index factory (manifest-driven backends).

use crate::manifest::VectorIndexSection;
use crate::repo::Repository;
use std::path::Path;
use wisp_index::{VectorBackendConfig, VectorDriver, VectorIndexBackend};
use wisp_model::{Result, WispError};

pub fn open_index(
    wisp_dir: &Path,
    section: &VectorIndexSection,
    codec: &str,
) -> Result<Box<dyn VectorIndexBackend>> {
    let cfg = section
        .backends
        .get(codec)
        .cloned()
        .unwrap_or_else(|| VectorBackendConfig {
            driver: "sqlite".into(),
            path: None,
            url_env: None,
            command: None,
        });
    let driver = VectorDriver::parse(&cfg.driver)
        .ok_or_else(|| WispError::usage(format!("unknown vector_index driver {:?}", cfg.driver)))?;
    match driver {
        VectorDriver::Sqlite => {
            let path_override = cfg.path.as_deref().map(Path::new);
            Ok(Box::new(
                wisp_sub_embed::SqliteVectorBackend::open(wisp_dir, codec, path_override)?,
            ))
        }
        VectorDriver::Postgres => {
            let url_env = cfg
                .url_env
                .as_deref()
                .unwrap_or("WISP_VECTOR_DSN");
            #[cfg(feature = "postgres-vector")]
            {
                return wisp_vector_pg::open_from_env(codec, url_env);
            }
            #[cfg(not(feature = "postgres-vector"))]
            {
                let _ = url_env;
                return Err(WispError::not_implemented(
                    "postgres vector backend: build wisp with --features postgres-vector",
                ));
            }
        }
        VectorDriver::ExternalCommand => Err(WispError::not_implemented(
            "command vector backend: implement wisp_index::VectorIndexBackend in your product crate",
        )),
    }
}

pub fn purge_vectors_for_sections(
    repo: &Repository,
    sections: &[wisp_model::SectionId],
) -> Result<()> {
    if sections.is_empty() {
        return Ok(());
    }
    let codec = repo
        .manifest
        .codecs
        .default_embedding
        .as_deref()
        .unwrap_or("");
    if codec.is_empty() {
        return Ok(());
    }
    let wisp_dir = repo.wisp_dir.clone();
    let vector_index = repo.manifest.vector_index.clone();
    let mut idx = open_index(&wisp_dir, &vector_index, codec)?;
    idx.delete_sections(sections)?;
    idx.checkpoint()?;
    Ok(())
}
