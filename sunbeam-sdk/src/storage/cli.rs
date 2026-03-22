use clap::Subcommand;
use std::io::Write;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};

// ---------------------------------------------------------------------------
// Top-level StorageCommand
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum StorageCommand {
    /// Bucket operations.
    Bucket {
        #[command(subcommand)]
        action: BucketAction,
    },
    /// Object operations.
    Object {
        #[command(subcommand)]
        action: ObjectAction,
    },
}

// ---------------------------------------------------------------------------
// Bucket sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum BucketAction {
    /// List all buckets.
    List,
    /// Create a bucket.
    Create {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
    },
    /// Delete a bucket.
    Delete {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
    },
    /// Check if a bucket exists.
    Exists {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
    },
}

// ---------------------------------------------------------------------------
// Object sub-commands
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum ObjectAction {
    /// List objects in a bucket.
    List {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Filter by key prefix.
        #[arg(long)]
        prefix: Option<String>,
        /// Maximum number of keys to return.
        #[arg(long)]
        max_keys: Option<u32>,
    },
    /// Download an object.
    Get {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Object key.
        #[arg(short, long)]
        key: String,
        /// Write to file instead of stdout.
        #[arg(long)]
        output_file: Option<String>,
    },
    /// Upload an object.
    Put {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Object key.
        #[arg(short, long)]
        key: String,
        /// Content-Type header.
        #[arg(long, default_value = "application/octet-stream")]
        content_type: String,
        /// Path to the file to upload.
        #[arg(short, long)]
        file: String,
    },
    /// Delete an object.
    Delete {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Object key.
        #[arg(short, long)]
        key: String,
    },
    /// Check if an object exists.
    Exists {
        /// Bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Object key.
        #[arg(short, long)]
        key: String,
    },
    /// Copy an object.
    Copy {
        /// Destination bucket name.
        #[arg(short, long)]
        bucket: String,
        /// Destination object key.
        #[arg(short, long)]
        key: String,
        /// Source in the form `/source-bucket/source-key`.
        #[arg(short, long)]
        source: String,
    },
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub async fn dispatch(
    cmd: StorageCommand,
    client: &SunbeamClient,
    output: OutputFormat,
) -> Result<()> {
    match cmd {
        StorageCommand::Bucket { action } => dispatch_bucket(action, client, output).await,
        StorageCommand::Object { action } => dispatch_object(action, client, output).await,
    }
}

// ---------------------------------------------------------------------------
// Bucket dispatch
// ---------------------------------------------------------------------------

async fn dispatch_bucket(
    action: BucketAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let s3 = client.s3().await?;
    match action {
        BucketAction::List => {
            let resp = s3.list_buckets().await?;
            output::render_list(
                &resp.buckets,
                &["NAME", "CREATED"],
                |b| {
                    vec![
                        b.name.clone(),
                        b.creation_date.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        BucketAction::Create { bucket } => {
            s3.create_bucket(&bucket).await?;
            output::ok(&format!("Created bucket {bucket}"));
            Ok(())
        }
        BucketAction::Delete { bucket } => {
            s3.delete_bucket(&bucket).await?;
            output::ok(&format!("Deleted bucket {bucket}"));
            Ok(())
        }
        BucketAction::Exists { bucket } => {
            let exists = s3.head_bucket(&bucket).await?;
            output::render(&serde_json::json!({ "exists": exists }), fmt)
        }
    }
}

// ---------------------------------------------------------------------------
// Object dispatch
// ---------------------------------------------------------------------------

async fn dispatch_object(
    action: ObjectAction,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let s3 = client.s3().await?;
    match action {
        ObjectAction::List {
            bucket,
            prefix,
            max_keys,
        } => {
            let resp = s3
                .list_objects_v2(&bucket, prefix.as_deref(), max_keys)
                .await?;
            output::render_list(
                &resp.contents,
                &["KEY", "SIZE", "LAST_MODIFIED", "ETAG"],
                |o| {
                    vec![
                        o.key.clone(),
                        o.size.map_or("-".into(), |s| s.to_string()),
                        o.last_modified.clone().unwrap_or_default(),
                        o.etag.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
        ObjectAction::Get {
            bucket,
            key,
            output_file,
        } => {
            let data = s3.get_object(&bucket, &key).await?;
            match output_file {
                Some(path) => {
                    std::fs::write(&path, &data)?;
                    output::ok(&format!("Written {} bytes to {path}", data.len()));
                }
                None => {
                    std::io::stdout().write_all(&data)?;
                }
            }
            Ok(())
        }
        ObjectAction::Put {
            bucket,
            key,
            content_type,
            file,
        } => {
            let data = bytes::Bytes::from(std::fs::read(&file)?);
            s3.put_object(&bucket, &key, &content_type, data).await?;
            output::ok(&format!("Uploaded {file} to {bucket}/{key}"));
            Ok(())
        }
        ObjectAction::Delete { bucket, key } => {
            s3.delete_object(&bucket, &key).await?;
            output::ok(&format!("Deleted {bucket}/{key}"));
            Ok(())
        }
        ObjectAction::Exists { bucket, key } => {
            let exists = s3.head_object(&bucket, &key).await?;
            output::render(&serde_json::json!({ "exists": exists }), fmt)
        }
        ObjectAction::Copy {
            bucket,
            key,
            source,
        } => {
            s3.copy_object(&bucket, &key, &source).await?;
            output::ok(&format!("Copied {source} to {bucket}/{key}"));
            Ok(())
        }
    }
}
