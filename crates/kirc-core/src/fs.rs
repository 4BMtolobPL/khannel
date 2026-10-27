use crate::memento::Memento;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::fs;
use std::path::Path;
use tracing::debug;

pub(crate) fn load<T, A>(path: &Path) -> anyhow::Result<T>
where
    T: Memento<A> + DeserializeOwned + Default,
{
    debug!(path = %path.display(), "Load");
    if !path.exists() {
        return Ok(T::default());
    }

    let contents = fs::read_to_string(path)?;
    let snapshot = serde_json::from_str(&contents)?;
    Ok(snapshot)
}

pub(crate) fn save<T, A>(path: &Path, snapshot: T) -> anyhow::Result<()>
where
    T: Memento<A> + Serialize,
{
    debug!(path = %path.display(), "Save");
    let tmp_path = path.with_extension("tmp");
    let data = serde_json::to_string(&snapshot)?;

    fs::write(&tmp_path, data)?;
    fs::rename(&tmp_path, path)?;

    Ok(())
}
