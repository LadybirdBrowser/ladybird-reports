use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::{
    domain::{
        AttachmentId, AttachmentManifest, AttachmentMediaType, AttachmentReference,
        IngestionLimits, ReportId, UploadId,
    },
    error::{AppError, Result},
};

#[derive(Clone)]
pub struct FileAttachmentStore {
    root: PathBuf,
    png_validation_memory: std::sync::Arc<Semaphore>,
}

pub struct StagingUpload {
    root: PathBuf,
    upload_id: UploadId,
    remove_on_drop: bool,
    png_validation_memory: std::sync::Arc<Semaphore>,
}

const PNG_VALIDATION_MEMORY_UNIT: usize = 1024 * 1024;
// A weighted semaphore limits the aggregate output reservation across all blocking PNG decoders in this process. With
// the 64 MiB hard per-image cap, no more than two maximum-sized decodes can run at once.
const PNG_VALIDATION_MEMORY_BUDGET: usize = 128 * 1024 * 1024;

/// Text attachments are stored gzip-compressed and read back uncompressed. Stored files that predate this are plain
/// text, which can be told apart: valid UTF-8 never starts with these bytes.
const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];
/// The most an attachment may hold uncompressed, as the database allows.
const MAXIMUM_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

pub struct AttachmentWriter {
    file: tokio::fs::File,
    path: PathBuf,
    maximum_bytes: u64,
    bytes_written: u64,
    digest: Sha256,
}

impl FileAttachmentStore {
    pub async fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();

        tokio::fs::create_dir_all(root.join("staging")).await?;
        tokio::fs::create_dir_all(root.join("reports")).await?;

        Ok(Self {
            root,
            png_validation_memory: std::sync::Arc::new(Semaphore::new(
                PNG_VALIDATION_MEMORY_BUDGET / PNG_VALIDATION_MEMORY_UNIT,
            )),
        })
    }

    pub fn available_space(&self) -> Result<u64> {
        Ok(fs2::available_space(&self.root)?)
    }

    pub async fn begin_staging(&self, upload_id: UploadId) -> Result<StagingUpload> {
        let path = self.staging_path(upload_id);

        match tokio::fs::create_dir(&path).await {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(AppError::Conflict("Upload staging area already exists"));
            }
            Err(error) => return Err(error.into()),
        }

        Ok(StagingUpload {
            root: self.root.clone(),
            upload_id,
            remove_on_drop: true,
            png_validation_memory: self.png_validation_memory.clone(),
        })
    }

    pub async fn finalize(&self, upload_id: UploadId, report_id: ReportId) -> Result<()> {
        let staging_path = self.staging_path(upload_id);
        let report_path = self.report_path(report_id);

        if tokio::fs::try_exists(&report_path).await? {
            // An interrupted request may retry after the atomic rename. The final directory is authoritative when the
            // staging directory is absent.
            if !tokio::fs::try_exists(&staging_path).await? {
                return Ok(());
            }

            return Err(AppError::Conflict(
                "Report attachment directory already exists",
            ));
        }

        tokio::fs::rename(staging_path, report_path).await?;
        sync_directory(self.root.join("reports")).await
    }

    /// The contents of an attachment, uncompressed whether or not it is stored compressed.
    pub async fn read(&self, storage_key: &str) -> Result<Vec<u8>> {
        let path = self.path_for_storage_key(storage_key)?;
        tokio::task::spawn_blocking(move || read_uncompressed(&path))
            .await
            .map_err(AppError::internal)?
    }

    /// Compresses an attachment that is stored as plain text, for those stored before text was compressed. Returns
    /// whether it did; an attachment that is compressed already, or gone, is left alone.
    pub async fn compress_stored_text(&self, storage_key: &str) -> Result<bool> {
        let path = self.path_for_storage_key(storage_key)?;
        let result = tokio::task::spawn_blocking(move || compress_in_place(&path))
            .await
            .map_err(AppError::internal)?;

        match result {
            Err(AppError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            result => result,
        }
    }

    pub async fn remove_staging(&self, upload_id: UploadId) -> Result<()> {
        let path = self.staging_path(upload_id);

        if tokio::fs::try_exists(&path).await? {
            tokio::fs::remove_dir_all(path).await?;
        }

        Ok(())
    }

    pub async fn remove_report(&self, report_id: ReportId) -> Result<()> {
        let path = self.report_path(report_id);

        if tokio::fs::try_exists(&path).await? {
            tokio::fs::remove_dir_all(path).await?;
        }

        Ok(())
    }

    pub async fn stale_staging_uploads(
        &self,
        older_than: std::time::SystemTime,
    ) -> Result<Vec<UploadId>> {
        let mut stale = Vec::new();
        let mut directory = tokio::fs::read_dir(self.root.join("staging")).await?;

        while let Some(entry) = directory.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }

            let Some(upload_id) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<UploadId>().ok())
            else {
                continue;
            };

            let modified = entry.metadata().await?.modified()?;
            if modified <= older_than {
                stale.push(upload_id);
            }
        }

        Ok(stale)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn staging_path(&self, upload_id: UploadId) -> PathBuf {
        self.root.join("staging").join(upload_id.to_string())
    }

    fn report_path(&self, report_id: ReportId) -> PathBuf {
        self.root.join("reports").join(report_id.to_string())
    }

    fn path_for_storage_key(&self, storage_key: &str) -> Result<PathBuf> {
        let mut components = storage_key.split('/');

        let prefix = components.next();
        let report_id = components.next();
        let attachment_id = components.next();

        let valid = prefix == Some("reports")
            && report_id
                .and_then(|value| value.parse::<ReportId>().ok())
                .is_some()
            && attachment_id
                .and_then(|value| value.parse::<AttachmentId>().ok())
                .is_some()
            && components.next().is_none();

        if !valid {
            return Err(AppError::Internal(anyhow::anyhow!(
                "invalid attachment storage key"
            )));
        }

        Ok(self.root.join(storage_key))
    }
}

impl StagingUpload {
    pub async fn create_writer(
        &self,
        attachment_id: AttachmentId,
        maximum_bytes: u64,
    ) -> Result<AttachmentWriter> {
        let path = self.path(attachment_id);
        let file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .await?;

        Ok(AttachmentWriter {
            file,
            path,
            maximum_bytes,
            bytes_written: 0,
            digest: Sha256::new(),
        })
    }

    pub async fn validate(
        &self,
        attachments: &[AttachmentManifest],
        attachment_ids: &HashMap<AttachmentReference, AttachmentId>,
        limits: &IngestionLimits,
    ) -> Result<()> {
        let mut validated_ids = HashSet::new();

        for attachment in attachments {
            let attachment_id = attachment_ids[&attachment.id];
            validate_attachment_file(
                &self.path(attachment_id),
                attachment,
                limits,
                self.validation_memory(),
            )
            .await?;
            validated_ids.insert(attachment_id);
        }

        let mut directory = tokio::fs::read_dir(self.directory()).await?;

        while let Some(entry) = directory.next_entry().await? {
            let file_name = entry.file_name();
            let file_name = file_name
                .to_str()
                .ok_or(AppError::InvalidRequest("Invalid attachment file name"))?;
            let attachment_id = file_name
                .parse::<AttachmentId>()
                .map_err(|_| AppError::InvalidRequest("Unexpected staged attachment"))?;

            if !validated_ids.contains(&attachment_id) {
                return Err(AppError::InvalidRequest("Unexpected staged attachment"));
            }
        }

        sync_directory(self.directory()).await
    }

    fn directory(&self) -> PathBuf {
        self.root.join("staging").join(self.upload_id.to_string())
    }

    fn path(&self, attachment_id: AttachmentId) -> PathBuf {
        self.directory().join(attachment_id.to_string())
    }

    fn validation_memory(&self) -> std::sync::Arc<Semaphore> {
        self.png_validation_memory.clone()
    }

    pub fn mark_referenced(&mut self) {
        self.remove_on_drop = false;
    }
}

impl Drop for StagingUpload {
    fn drop(&mut self) {
        if !self.remove_on_drop {
            return;
        }

        let path = self.directory();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if let Err(error) = tokio::fs::remove_dir_all(&path).await
                    && error.kind() != std::io::ErrorKind::NotFound
                {
                    tracing::warn!(?error, ?path, "Could not remove dropped upload staging");
                }
            });
        }
    }
}

impl AttachmentWriter {
    pub async fn write(&mut self, bytes: &[u8]) -> Result<()> {
        let new_size = self
            .bytes_written
            .checked_add(bytes.len() as u64)
            .ok_or(AppError::PayloadTooLarge)?;

        if new_size > self.maximum_bytes {
            return Err(AppError::PayloadTooLarge);
        }

        self.file.write_all(bytes).await?;
        self.digest.update(bytes);
        self.bytes_written = new_size;

        Ok(())
    }

    pub async fn finish(mut self, expected_size: u64, expected_digest: &str) -> Result<()> {
        self.file.flush().await?;
        self.file.sync_all().await?;

        let actual_digest = hex::encode(self.digest.finalize());

        if self.bytes_written != expected_size || actual_digest != expected_digest {
            let _ = tokio::fs::remove_file(&self.path).await;
            return Err(AppError::InvalidRequest(
                "Attachment size or digest does not match its manifest",
            ));
        }

        Ok(())
    }
}

async fn validate_attachment_file(
    path: &Path,
    attachment: &AttachmentManifest,
    limits: &IngestionLimits,
    validation_memory: std::sync::Arc<Semaphore>,
) -> Result<()> {
    let path = path.to_owned();
    let attachment = attachment.clone();
    let limits = limits.clone();

    let memory_permits = limits
        .png_decoded_bytes
        .div_ceil(PNG_VALIDATION_MEMORY_UNIT) as u32;
    let _memory = match attachment.media_type {
        AttachmentMediaType::Png => Some(
            validation_memory
                .acquire_many_owned(memory_permits)
                .await
                .map_err(|_| AppError::Unavailable)?,
        ),
        AttachmentMediaType::PlainText => None,
    };

    tokio::task::spawn_blocking(move || match attachment.media_type {
        // Text is stored compressed once it is known to be valid.
        AttachmentMediaType::PlainText => {
            validate_utf8_text(&path, limits.attachment_bytes)?;
            compress_in_place(&path).map(drop)
        }
        AttachmentMediaType::Png => {
            validate_png(&path, limits.png_pixels, limits.png_decoded_bytes)
        }
    })
    .await
    .map_err(AppError::internal)?
}

fn read_uncompressed(path: &Path) -> Result<Vec<u8>> {
    let bytes = std::fs::read(path)?;
    if !bytes.starts_with(&GZIP_MAGIC) {
        return Ok(bytes);
    }

    // The limit is for the sake of a damaged file, which could otherwise claim to hold far more than it was given.
    let mut uncompressed = Vec::new();
    GzDecoder::new(bytes.as_slice())
        .take(MAXIMUM_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut uncompressed)?;
    if uncompressed.len() as u64 > MAXIMUM_ATTACHMENT_BYTES {
        return Err(AppError::Internal(anyhow::anyhow!(
            "stored attachment is larger than an attachment may be"
        )));
    }

    Ok(uncompressed)
}

/// Replaces a plain text file with its gzip-compressed contents, through a file beside it that is renamed over it so
/// a reader sees one or the other. Returns whether the file was plain text.
fn compress_in_place(path: &Path) -> Result<bool> {
    let bytes = std::fs::read(path)?;
    if bytes.starts_with(&GZIP_MAGIC) {
        return Ok(false);
    }

    let mut temporary_name = path.as_os_str().to_owned();
    temporary_name.push(".compressing");
    let temporary = PathBuf::from(temporary_name);

    let compressed = (|| -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        let mut encoder = GzEncoder::new(std::io::BufWriter::new(file), Compression::default());
        encoder.write_all(&bytes)?;
        let file = encoder
            .finish()?
            .into_inner()
            .map_err(|error| error.into_error())?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();

    if let Err(error) = compressed {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }

    Ok(true)
}

fn validate_utf8_text(path: &Path, maximum_bytes: usize) -> Result<()> {
    let bytes = std::fs::read(path)?;

    if bytes.len() > maximum_bytes || std::str::from_utf8(&bytes).is_err() {
        return Err(AppError::InvalidRequest(
            "Text attachment is not valid bounded UTF-8",
        ));
    }

    Ok(())
}

fn validate_png(path: &Path, maximum_pixels: u64, maximum_decoded_bytes: usize) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));

    decoder.set_limits(png::Limits {
        bytes: maximum_decoded_bytes,
    });

    let mut reader = decoder
        .read_info()
        .map_err(|_| AppError::InvalidRequest("Invalid PNG attachment"))?;

    let info = reader.info();
    let pixels = u64::from(info.width)
        .checked_mul(u64::from(info.height))
        .ok_or(AppError::InvalidRequest("PNG dimensions are too large"))?;

    if info.width == 0
        || info.height == 0
        || pixels > maximum_pixels
        || info.animation_control.is_some()
    {
        return Err(AppError::InvalidRequest(
            "PNG dimensions or animation are not supported",
        ));
    }

    let buffer_size = reader
        .output_buffer_size()
        .ok_or(AppError::InvalidRequest("PNG output is too large"))?;

    if buffer_size > maximum_decoded_bytes {
        return Err(AppError::InvalidRequest("PNG output is too large"));
    }

    let mut output = vec![0; buffer_size];
    reader
        .next_frame(&mut output)
        .map_err(|_| AppError::InvalidRequest("Invalid PNG pixel data"))?;
    reader
        .finish()
        .map_err(|_| AppError::InvalidRequest("Invalid PNG ending"))?;

    Ok(())
}

async fn sync_directory(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || std::fs::File::open(path)?.sync_all())
        .await
        .map_err(AppError::internal)??;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn text_attachments_must_be_utf8_and_bounded() {
        let mut file = tempfile::NamedTempFile::new().expect("create temporary attachment");
        file.write_all(b"symbolicated stack")
            .expect("write attachment");

        assert!(validate_utf8_text(file.path(), 64).is_ok());
        assert!(validate_utf8_text(file.path(), 4).is_err());

        file.as_file_mut().set_len(0).expect("truncate attachment");
        file.write_all(&[0xff, 0xfe]).expect("write invalid UTF-8");
        assert!(validate_utf8_text(file.path(), 64).is_err());
    }

    #[test]
    fn arbitrary_image_bytes_are_not_accepted_as_png() {
        let mut file = tempfile::NamedTempFile::new().expect("create temporary attachment");
        file.write_all(b"not a PNG").expect("write attachment");

        assert!(validate_png(file.path(), 100, 1024).is_err());
    }

    #[test]
    fn png_output_must_fit_the_decoded_byte_budget() {
        let mut file = tempfile::NamedTempFile::new().expect("create temporary attachment");
        {
            let mut encoder = png::Encoder::new(file.as_file_mut(), 2_048, 2_048);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Sixteen);
            let _writer = encoder.write_header().expect("write PNG header");
        }
        file.flush().expect("flush PNG header");

        // The header describes a 32 MiB frame, so validation must reject it before allocating the output buffer or
        // reading compressed pixels.
        assert!(validate_png(file.path(), 5_000_000, 1024 * 1024).is_err());
    }

    /// An attachment file in a store, where the store looks for it: `reports/<report>/<attachment>`.
    async fn stored_file(
        directory: &Path,
        contents: &[u8],
    ) -> (FileAttachmentStore, String, PathBuf) {
        let store = FileAttachmentStore::open(directory)
            .await
            .expect("open attachment store");
        let report_id = ReportId::new();
        let attachment_id = AttachmentId::new();
        let key = format!("reports/{report_id}/{attachment_id}");
        let path = directory.join(&key);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .expect("create report directory");
        tokio::fs::write(&path, contents)
            .await
            .expect("write attachment");

        (store, key, path)
    }

    fn diagnostics() -> Vec<u8> {
        "Layout tree:\n  BlockContainer <body> at [0,0] [0+0+0 1104 0+0+0]\n"
            .repeat(2_000)
            .into_bytes()
    }

    #[tokio::test]
    async fn text_that_is_stored_plain_is_read_back_and_compressed_once() {
        let directory = tempfile::tempdir().expect("create attachment root");
        let text = diagnostics();
        let (store, key, path) = stored_file(directory.path(), &text).await;

        // Stored before text was compressed: it reads as it is.
        assert_eq!(store.read(&key).await.expect("read plain text"), text);

        assert!(store.compress_stored_text(&key).await.expect("compress"));
        let stored = tokio::fs::read(&path).await.expect("read stored file");
        assert!(stored.starts_with(&GZIP_MAGIC));
        assert!(
            stored.len() < text.len() / 10,
            "{} of {}",
            stored.len(),
            text.len()
        );
        assert_eq!(store.read(&key).await.expect("read compressed text"), text);

        // Nothing left to do, and nothing left behind.
        assert!(
            !store
                .compress_stored_text(&key)
                .await
                .expect("compress again")
        );
        assert_eq!(tokio::fs::read(&path).await.expect("read again"), stored);
        let mut siblings = tokio::fs::read_dir(path.parent().unwrap()).await.unwrap();
        assert!(siblings.next_entry().await.unwrap().is_some());
        assert!(siblings.next_entry().await.unwrap().is_none());
    }

    #[tokio::test]
    async fn an_attachment_that_is_gone_is_not_compressed() {
        let directory = tempfile::tempdir().expect("create attachment root");
        let (store, key, path) = stored_file(directory.path(), b"text").await;
        tokio::fs::remove_file(&path)
            .await
            .expect("remove attachment");

        assert!(
            !store
                .compress_stored_text(&key)
                .await
                .expect("compress missing")
        );
    }

    #[tokio::test]
    async fn validated_text_is_stored_compressed_and_images_are_left_alone() {
        let directory = tempfile::tempdir().expect("create attachment root");
        let text = diagnostics();
        let (store, key, path) = stored_file(directory.path(), &text).await;
        let manifest = |media_type: &str, size: usize| -> AttachmentManifest {
            serde_json::from_value(serde_json::json!({
                "id": "diagnostics",
                "name": "diagnostics.txt",
                "media_type": media_type,
                "size": size,
                "sha256": "0".repeat(64),
            }))
            .expect("attachment manifest")
        };
        let limits = IngestionLimits::default();
        let memory = std::sync::Arc::new(Semaphore::new(1_024));

        validate_attachment_file(
            &path,
            &manifest("text/plain", text.len()),
            &limits,
            memory.clone(),
        )
        .await
        .expect("validate text");
        let stored = tokio::fs::read(&path).await.expect("read stored file");
        assert!(stored.starts_with(&GZIP_MAGIC));
        assert_eq!(store.read(&key).await.expect("read text"), text);

        // Text that is not valid is rejected before anything is changed.
        tokio::fs::write(&path, [0xff, 0xfe])
            .await
            .expect("write invalid text");
        assert!(
            validate_attachment_file(&path, &manifest("text/plain", 2), &limits, memory.clone())
                .await
                .is_err()
        );
        assert_eq!(
            tokio::fs::read(&path).await.expect("read invalid text"),
            [0xff, 0xfe]
        );

        // An image is not text, so it stays as it was uploaded.
        let image = b"\x89PNG not really".to_vec();
        tokio::fs::write(&path, &image).await.expect("write image");
        let _ =
            validate_attachment_file(&path, &manifest("image/png", image.len()), &limits, memory)
                .await;
        assert_eq!(tokio::fs::read(&path).await.expect("read image"), image);
    }

    #[tokio::test]
    async fn a_stored_file_that_claims_too_much_is_not_read() {
        let directory = tempfile::tempdir().expect("create attachment root");
        let (store, key, path) = stored_file(directory.path(), b"x").await;
        let bomb = {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
            let chunk = vec![b'a'; 1024 * 1024];
            for _ in 0..=64 {
                encoder.write_all(&chunk).expect("compress chunk");
            }
            encoder.finish().expect("finish compression")
        };
        tokio::fs::write(&path, bomb).await.expect("write bomb");

        assert!(store.read(&key).await.is_err());
    }

    #[tokio::test]
    async fn dropped_staging_uploads_are_removed() {
        let directory = tempfile::tempdir().expect("create attachment root");
        let store = FileAttachmentStore::open(directory.path())
            .await
            .expect("open attachment store");
        let upload_id = UploadId::new();
        let staging_path = directory.path().join("staging").join(upload_id.to_string());

        let staging = store
            .begin_staging(upload_id)
            .await
            .expect("create staging upload");
        assert!(tokio::fs::try_exists(&staging_path).await.unwrap());
        drop(staging);

        for _ in 0..20 {
            if !tokio::fs::try_exists(&staging_path).await.unwrap() {
                return;
            }
            tokio::task::yield_now().await;
        }

        panic!("dropped staging directory still exists");
    }
}
