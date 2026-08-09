use super::types::{RecordChunk, SyncCollection};
use super::webdav_client::WebdavClient;

pub fn chunk_path(collection: SyncCollection, chunk: u32) -> String {
    format!("{}/chunks/chunk_{:03}.json", collection.dir(), chunk)
}

pub async fn load_chunk(
    client: &WebdavClient,
    collection: SyncCollection,
    chunk: u32,
) -> Result<RecordChunk, String> {
    Ok(client
        .get_json(&chunk_path(collection, chunk))
        .await?
        .unwrap_or_default())
}

pub async fn save_chunk(
    client: &WebdavClient,
    collection: SyncCollection,
    chunk: u32,
    data: &RecordChunk,
) -> Result<(), String> {
    client.put_json(&chunk_path(collection, chunk), data).await
}

#[cfg(test)]
mod tests {
    use super::chunk_path;
    use crate::services::webdav_sync::types::SyncCollection;

    #[test]
    fn chunk_path_layout_is_stable() {
        assert_eq!(chunk_path(SyncCollection::History, 0), "history/chunks/chunk_000.json");
        assert_eq!(chunk_path(SyncCollection::History, 12), "history/chunks/chunk_012.json");
        assert_eq!(chunk_path(SyncCollection::Favorites, 0), "favorites/chunks/chunk_000.json");
        assert_eq!(chunk_path(SyncCollection::Favorites, 500), "favorites/chunks/chunk_500.json");
    }
}
