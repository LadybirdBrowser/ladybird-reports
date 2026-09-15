use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{
    domain::{
        AttachmentId, AttachmentManifest, AttachmentMediaType, IngestionLimits, ReportId, UploadId,
    },
    error::{AppError, Result},
};

#[derive(Clone)]
pub struct FileAttachmentStore {
    root: PathBuf,
}

pub struct StagingUpload {
    root: PathBuf,
    upload_id: UploadId,
}

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

        Ok(Self { root })
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
        })
    }

    pub async fn finalize(&self, upload_id: UploadId, report_id: ReportId) -> Result<()> {
        let staging_path = self.staging_path(upload_id);
        let report_path = self.report_path(report_id);

        if tokio::fs::try_exists(&report_path).await? {
            // An interrupted request may retry after the atomic rename. The final
            // directory is authoritative when the staging directory is absent.
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

    pub async fn read(&self, storage_key: &str) -> Result<Vec<u8>> {
        let path = self.path_for_storage_key(storage_key)?;
        Ok(tokio::fs::read(path).await?)
    }

    pub async fn remove_staging(&self, upload_id: UploadId) -> Result<()> {
        let path = self.staging_path(upload_id);

        if tokio::fs::try_exists(&path).await? {
            tokio::fs::remove_dir_all(path).await?;
        }

        Ok(())
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
        limits: &IngestionLimits,
    ) -> Result<()> {
        let mut validated_ids = HashSet::new();

        for attachment in attachments {
            validate_attachment_file(&self.path(attachment.id), attachment, limits).await?;
            validated_ids.insert(attachment.id);
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
) -> Result<()> {
    let path = path.to_owned();
    let attachment = attachment.clone();
    let limits = limits.clone();

    tokio::task::spawn_blocking(move || match attachment.media_type {
        AttachmentMediaType::PlainText => validate_utf8_text(&path, limits.attachment_bytes),
        AttachmentMediaType::Png => validate_png(&path, limits.png_pixels),
    })
    .await
    .map_err(|error| AppError::Internal(error.into()))?
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

fn validate_png(path: &Path, maximum_pixels: u64) -> Result<()> {
    let file = std::fs::File::open(path)?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));

    decoder.set_limits(png::Limits {
        bytes: 256 * 1024 * 1024,
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

    if buffer_size > 256 * 1024 * 1024 {
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
        .map_err(|error| AppError::Internal(error.into()))??;

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

        assert!(validate_png(file.path(), 100).is_err());
    }
}
