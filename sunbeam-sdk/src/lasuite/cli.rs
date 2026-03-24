//! CLI command definitions and dispatch for all 7 La Suite services.

use clap::Subcommand;

use crate::client::SunbeamClient;
use crate::error::Result;
use crate::output::{self, OutputFormat};


// ═══════════════════════════════════════════════════════════════════════════
// People
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum PeopleCommand {
    /// Contact management.
    Contact {
        #[command(subcommand)]
        action: ContactAction,
    },
    /// Team management.
    Team {
        #[command(subcommand)]
        action: TeamAction,
    },
    /// Service provider listing.
    ServiceProvider {
        #[command(subcommand)]
        action: ServiceProviderAction,
    },
    /// Mail domain listing.
    MailDomain {
        #[command(subcommand)]
        action: MailDomainAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ContactAction {
    /// List contacts.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Get a contact by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a contact from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a contact from JSON.
    Update {
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a contact.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TeamAction {
    /// List teams.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Get a team by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a team from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceProviderAction {
    /// List service providers.
    List,
}

#[derive(Subcommand, Debug)]
pub enum MailDomainAction {
    /// List mail domains.
    List,
}

pub async fn dispatch_people(
    cmd: PeopleCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let people = client.people().await?;
    match cmd {
        PeopleCommand::Contact { action } => match action {
            ContactAction::List { page } => {
                let page_data = people.list_contacts(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "EMAIL", "ORGANIZATION"],
                    |c| {
                        vec![
                            c.id.clone(),
                            format!(
                                "{} {}",
                                c.first_name.as_deref().unwrap_or(""),
                                c.last_name.as_deref().unwrap_or("")
                            )
                            .trim()
                            .to_string(),
                            c.email.clone().unwrap_or_default(),
                            c.organization.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            ContactAction::Get { id } => {
                let item = people.get_contact(&id).await?;
                output::render(&item, fmt)
            }
            ContactAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = people.create_contact(&json).await?;
                output::render(&item, fmt)
            }
            ContactAction::Update { id, data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = people.update_contact(&id, &json).await?;
                output::render(&item, fmt)
            }
            ContactAction::Delete { id } => {
                people.delete_contact(&id).await?;
                output::ok(&format!("Deleted contact {id}"));
                Ok(())
            }
        },
        PeopleCommand::Team { action } => match action {
            TeamAction::List { page } => {
                let page_data = people.list_teams(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "DESCRIPTION"],
                    |t| {
                        vec![
                            t.id.clone(),
                            t.name.clone().unwrap_or_default(),
                            t.description.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            TeamAction::Get { id } => {
                let item = people.get_team(&id).await?;
                output::render(&item, fmt)
            }
            TeamAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = people.create_team(&json).await?;
                output::render(&item, fmt)
            }
        },
        PeopleCommand::ServiceProvider { action } => match action {
            ServiceProviderAction::List => {
                let page_data = people.list_service_providers().await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "BASE_URL"],
                    |sp| {
                        vec![
                            sp.id.clone(),
                            sp.name.clone().unwrap_or_default(),
                            sp.base_url.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
        },
        PeopleCommand::MailDomain { action } => match action {
            MailDomainAction::List => {
                let page_data = people.list_mail_domains().await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "STATUS"],
                    |md| {
                        vec![
                            md.id.clone(),
                            md.name.clone().unwrap_or_default(),
                            md.status.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Docs
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum DocsCommand {
    /// Document management.
    Document {
        #[command(subcommand)]
        action: DocumentAction,
    },
    /// Template management.
    Template {
        #[command(subcommand)]
        action: TemplateAction,
    },
    /// Version history.
    Version {
        #[command(subcommand)]
        action: VersionAction,
    },
    /// Invite a user to a document.
    Invite {
        /// Document ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum DocumentAction {
    /// List documents.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Get a document by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a document from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a document from JSON.
    Update {
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a document.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum TemplateAction {
    /// List templates.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Create a template from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum VersionAction {
    /// List versions of a document.
    List {
        /// Document ID.
        #[arg(short, long)]
        id: String,
    },
}

pub async fn dispatch_docs(
    cmd: DocsCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let docs = client.docs().await?;
    match cmd {
        DocsCommand::Document { action } => match action {
            DocumentAction::List { page } => {
                let page_data = docs.list_documents(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "TITLE", "PUBLIC", "UPDATED"],
                    |d| {
                        vec![
                            d.id.clone(),
                            d.title.clone().unwrap_or_default(),
                            d.is_public.map_or("-".into(), |p| p.to_string()),
                            d.updated_at.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            DocumentAction::Get { id } => {
                let item = docs.get_document(&id).await?;
                output::render(&item, fmt)
            }
            DocumentAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = docs.create_document(&json).await?;
                output::render(&item, fmt)
            }
            DocumentAction::Update { id, data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = docs.update_document(&id, &json).await?;
                output::render(&item, fmt)
            }
            DocumentAction::Delete { id } => {
                docs.delete_document(&id).await?;
                output::ok(&format!("Deleted document {id}"));
                Ok(())
            }
        },
        DocsCommand::Template { action } => match action {
            TemplateAction::List { page } => {
                let page_data = docs.list_templates(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "TITLE", "PUBLIC"],
                    |t| {
                        vec![
                            t.id.clone(),
                            t.title.clone().unwrap_or_default(),
                            t.is_public.map_or("-".into(), |p| p.to_string()),
                        ]
                    },
                    fmt,
                )
            }
            TemplateAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = docs.create_template(&json).await?;
                output::render(&item, fmt)
            }
        },
        DocsCommand::Version { action } => match action {
            VersionAction::List { id } => {
                let page_data = docs.list_versions(&id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "VERSION", "CREATED"],
                    |v| {
                        vec![
                            v.id.clone(),
                            v.version_number.map_or("-".into(), |n| n.to_string()),
                            v.created_at.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
        },
        DocsCommand::Invite { id, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = docs.invite_user(&id, &json).await?;
            output::render(&item, fmt)
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Meet
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum MeetCommand {
    /// Room management.
    Room {
        #[command(subcommand)]
        action: RoomAction,
    },
    /// Recording management.
    Recording {
        #[command(subcommand)]
        action: RecordingAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum RoomAction {
    /// List rooms.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Get a room by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a room from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update a room from JSON.
    Update {
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a room.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum RecordingAction {
    /// List recordings for a room.
    List {
        /// Room ID.
        #[arg(short, long)]
        id: String,
    },
}

pub async fn dispatch_meet(
    cmd: MeetCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let meet = client.meet().await?;
    match cmd {
        MeetCommand::Room { action } => match action {
            RoomAction::List { page } => {
                let page_data = meet.list_rooms(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "SLUG", "PUBLIC"],
                    |r| {
                        vec![
                            r.id.clone(),
                            r.name.clone().unwrap_or_default(),
                            r.slug.clone().unwrap_or_default(),
                            r.is_public.map_or("-".into(), |p| p.to_string()),
                        ]
                    },
                    fmt,
                )
            }
            RoomAction::Get { id } => {
                let item = meet.get_room(&id).await?;
                output::render(&item, fmt)
            }
            RoomAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = meet.create_room(&json).await?;
                output::render(&item, fmt)
            }
            RoomAction::Update { id, data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = meet.update_room(&id, &json).await?;
                output::render(&item, fmt)
            }
            RoomAction::Delete { id } => {
                meet.delete_room(&id).await?;
                output::ok(&format!("Deleted room {id}"));
                Ok(())
            }
        },
        MeetCommand::Recording { action } => match action {
            RecordingAction::List { id } => {
                let page_data = meet.list_recordings(&id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "FILENAME", "DURATION", "CREATED"],
                    |r| {
                        vec![
                            r.id.clone(),
                            r.filename.clone().unwrap_or_default(),
                            r.duration.map_or("-".into(), |d| format!("{d:.1}s")),
                            r.created_at.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Drive
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum DriveCommand {
    /// File management.
    File {
        #[command(subcommand)]
        action: FileAction,
    },
    /// Folder management.
    Folder {
        #[command(subcommand)]
        action: FolderAction,
    },
    /// Share a file with a user.
    Share {
        /// File ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Permission management.
    Permission {
        #[command(subcommand)]
        action: PermissionAction,
    },
    /// Upload a local file or directory to a Drive folder.
    Upload {
        /// Local path to upload (file or directory).
        #[arg(short, long)]
        path: String,
        /// Target Drive folder ID.
        #[arg(short = 't', long)]
        folder_id: String,
        /// Number of concurrent uploads.
        #[arg(long, default_value = "4")]
        parallel: usize,
    },
}

#[derive(Subcommand, Debug)]
pub enum FileAction {
    /// List files.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Get a file by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Upload a file (JSON metadata).
    Upload {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete a file.
    Delete {
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum FolderAction {
    /// List folders.
    List {
        #[arg(long)]
        page: Option<u32>,
    },
    /// Create a folder from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PermissionAction {
    /// List permissions for a file.
    List {
        /// File ID.
        #[arg(short, long)]
        id: String,
    },
}

pub async fn dispatch_drive(
    cmd: DriveCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let drive = client.drive().await?;
    match cmd {
        DriveCommand::File { action } => match action {
            FileAction::List { page } => {
                let page_data = drive.list_files(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "TITLE", "TYPE", "SIZE", "MIMETYPE"],
                    |f| {
                        vec![
                            f.id.clone(),
                            f.title.clone().unwrap_or_default(),
                            f.item_type.clone().unwrap_or_default(),
                            f.size.map_or("-".into(), |s| s.to_string()),
                            f.mimetype.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            FileAction::Get { id } => {
                let item = drive.get_file(&id).await?;
                output::render(&item, fmt)
            }
            FileAction::Upload { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = drive.upload_file(&json).await?;
                output::render(&item, fmt)
            }
            FileAction::Delete { id } => {
                drive.delete_file(&id).await?;
                output::ok(&format!("Deleted file {id}"));
                Ok(())
            }
        },
        DriveCommand::Folder { action } => match action {
            FolderAction::List { page } => {
                let page_data = drive.list_folders(page).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "TITLE", "CHILDREN", "CREATED"],
                    |f| {
                        vec![
                            f.id.clone(),
                            f.title.clone().unwrap_or_default(),
                            f.numchild.map_or("-".into(), |n| n.to_string()),
                            f.created_at.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            FolderAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = drive.create_folder(&json).await?;
                output::render(&item, fmt)
            }
        },
        DriveCommand::Share { id, data } => {
            let json = output::read_json_input(data.as_deref())?;
            let item = drive.share_file(&id, &json).await?;
            output::render(&item, fmt)
        }
        DriveCommand::Permission { action } => match action {
            PermissionAction::List { id } => {
                let page_data = drive.get_permissions(&id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "USER_ID", "ROLE", "READ", "WRITE"],
                    |p| {
                        vec![
                            p.id.clone(),
                            p.user_id.clone().unwrap_or_default(),
                            p.role.clone().unwrap_or_default(),
                            p.can_read.map_or("-".into(), |v| v.to_string()),
                            p.can_write.map_or("-".into(), |v| v.to_string()),
                        ]
                    },
                    fmt,
                )
            }
        },
        DriveCommand::Upload { path, folder_id, parallel } => {
            upload_recursive(drive, &path, &folder_id, parallel).await
        }
    }
}

/// A file that needs uploading, collected during the directory-walk phase.
struct UploadJob {
    local_path: std::path::PathBuf,
    parent_id: String,
    file_size: u64,
    relative_path: String,
}

/// Recursively upload a local file or directory to a Drive folder.
async fn upload_recursive(
    drive: &super::DriveClient,
    local_path: &str,
    parent_id: &str,
    parallel: usize,
) -> Result<()> {
    use indicatif::{HumanBytes, MultiProgress, ProgressBar, ProgressStyle};
    use std::sync::Arc;
    use tokio::sync::Semaphore;

    let path = std::path::Path::new(local_path);
    if !path.exists() {
        return Err(crate::error::SunbeamError::Other(format!(
            "Path does not exist: {local_path}"
        )));
    }

    // Phase 1 — Walk and collect: create folders sequentially, gather file jobs.
    let mut jobs = Vec::new();
    if path.is_file() {
        let file_size = std::fs::metadata(path)
            .map_err(|e| crate::error::SunbeamError::Other(format!("stat: {e}")))?
            .len();
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unnamed");
        if !filename.starts_with('.') {
            jobs.push(UploadJob {
                local_path: path.to_path_buf(),
                parent_id: parent_id.to_string(),
                file_size,
                relative_path: filename.to_string(),
            });
        }
    } else if path.is_dir() {
        collect_upload_jobs(drive, path, parent_id, "", &mut jobs).await?;
    } else {
        return Err(crate::error::SunbeamError::Other(format!(
            "Not a file or directory: {local_path}"
        )));
    }

    if jobs.is_empty() {
        output::ok("Nothing to upload.");
        return Ok(());
    }

    let total_files = jobs.len() as u64;
    let total_bytes: u64 = jobs.iter().map(|j| j.file_size).sum();

    // Clear the folder creation line
    eprint!("\r\x1b[K");

    // Phase 2 — Parallel upload with progress bars.
    let multi = MultiProgress::new();

    // Overall bar tracks bytes (not file count) for accurate speed + ETA.
    let overall_style = ProgressStyle::with_template(
        " {spinner:.green} [{elapsed_precise}] {bar:40.cyan/blue} {pos}/{len} files  {msg}",
    )
    .unwrap()
    .progress_chars("━╸─");
    let overall = multi.add(ProgressBar::new(total_files));
    overall.set_style(overall_style);
    overall.enable_steady_tick(std::time::Duration::from_millis(100));

    // Byte-tracking bar (hidden) — drives speed + ETA calculation.
    let bytes_bar = multi.add(ProgressBar::hidden());
    bytes_bar.set_length(total_bytes);

    let file_style = ProgressStyle::with_template(
        " {spinner:.cyan} {wide_msg}",
    )
    .unwrap();

    let sem = Arc::new(Semaphore::new(parallel));
    let drive = Arc::new(drive.clone());
    let mut handles = Vec::new();
    let start = std::time::Instant::now();
    let completed_bytes = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let completed_files = Arc::new(std::sync::atomic::AtomicU64::new(0));

    // Spawn a task to update the overall bar message with speed + ETA.
    {
        let overall = overall.clone();
        let completed_bytes = Arc::clone(&completed_bytes);
        let completed_files = Arc::clone(&completed_files);
        let total_bytes = total_bytes;
        let total_files = total_files;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                let done_bytes = completed_bytes.load(std::sync::atomic::Ordering::Relaxed);
                let done_files = completed_files.load(std::sync::atomic::Ordering::Relaxed);
                let elapsed = start.elapsed().as_secs_f64();
                let speed = if elapsed > 0.5 { done_bytes as f64 / elapsed } else { 0.0 };
                let remaining_bytes = total_bytes.saturating_sub(done_bytes);
                let eta_secs = if speed > 0.0 { remaining_bytes as f64 / speed } else { 0.0 };
                let eta_min = eta_secs as u64 / 60;
                let eta_sec = eta_secs as u64 % 60;
                overall.set_message(format!(
                    "{}/{}  {}/s  ETA: {eta_min}m {eta_sec:02}s",
                    HumanBytes(done_bytes),
                    HumanBytes(total_bytes),
                    HumanBytes(speed as u64),
                ));
                overall.set_position(done_files);
                if done_files >= total_files {
                    break;
                }
            }
        });
    }

    for job in jobs {
        let permit = sem.clone().acquire_owned().await.unwrap();
        let drive = Arc::clone(&drive);
        let multi = multi.clone();
        let file_style = file_style.clone();
        let completed_bytes = Arc::clone(&completed_bytes);
        let completed_files = Arc::clone(&completed_files);
        let job_size = job.file_size;

        let handle = tokio::spawn(async move {
            let pb = multi.insert_before(&multi.add(ProgressBar::hidden()), ProgressBar::new_spinner());
            pb.set_style(file_style);
            pb.set_message(format!("⬆ {}", job.relative_path));
            pb.enable_steady_tick(std::time::Duration::from_millis(80));

            let result = upload_single_file_with_progress(&drive, &job, &pb).await;

            if result.is_ok() {
                pb.set_message(format!("✓ {}", job.relative_path));
            } else {
                pb.set_message(format!("✗ {}", job.relative_path));
            }
            pb.finish();
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            pb.finish_and_clear();
            multi.remove(&pb);

            completed_bytes.fetch_add(job_size, std::sync::atomic::Ordering::Relaxed);
            completed_files.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

            drop(permit);
            result
        });
        handles.push(handle);
    }

    let mut errors = 0u64;
    for handle in handles {
        match handle.await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                errors += 1;
                multi.suspend(|| eprintln!("    ERROR: {e}"));
            }
            Err(e) => {
                errors += 1;
                multi.suspend(|| eprintln!("    ERROR: task panic: {e}"));
            }
        }
    }

    overall.finish_and_clear();
    multi.clear().ok();

    let elapsed = start.elapsed();
    let secs = elapsed.as_secs_f64();
    let speed = if secs > 0.0 {
        total_bytes as f64 / secs
    } else {
        0.0
    };
    let mins = elapsed.as_secs() / 60;
    let secs_rem = elapsed.as_secs() % 60;
    let uploaded = total_files - errors;
    if errors > 0 {
        println!(
            "✓ Uploaded {uploaded}/{total_files} files ({}) in {mins}m {secs_rem}s ({}/s) — {errors} failed",
            HumanBytes(total_bytes),
            HumanBytes(speed as u64),
        );
    } else {
        println!(
            "✓ Uploaded {total_files} files ({}) in {mins}m {secs_rem}s ({}/s)",
            HumanBytes(total_bytes),
            HumanBytes(speed as u64),
        );
    }

    Ok(())
}

/// Phase 1: Walk a directory recursively, create folders in Drive sequentially,
/// and collect [`UploadJob`]s for every regular file.
async fn collect_upload_jobs(
    drive: &super::DriveClient,
    dir: &std::path::Path,
    parent_id: &str,
    prefix: &str,
    jobs: &mut Vec<UploadJob>,
) -> Result<()> {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    // Skip hidden directories
    if dir_name.starts_with('.') {
        return Ok(());
    }

    // Build the display prefix for children
    let display_prefix = if prefix.is_empty() {
        dir_name.to_string()
    } else {
        format!("{prefix}/{dir_name}")
    };

    eprint!("\r\x1b[K  Creating folder: {display_prefix}  ");

    let folder = drive
        .create_child(
            parent_id,
            &serde_json::json!({
                "title": dir_name,
                "type": "folder",
            }),
        )
        .await?;

    let folder_id = folder["id"]
        .as_str()
        .ok_or_else(|| crate::error::SunbeamError::Other("No folder ID in response".into()))?
        .to_string();

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| crate::error::SunbeamError::Other(format!("reading dir: {e}")))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let entry_path = entry.path();
        let name = entry
            .file_name()
            .to_str()
            .unwrap_or_default()
            .to_string();

        // Skip hidden entries
        if name.starts_with('.') {
            continue;
        }

        if entry_path.is_dir() {
            Box::pin(collect_upload_jobs(
                drive,
                &entry_path,
                &folder_id,
                &display_prefix,
                jobs,
            ))
            .await?;
        } else if entry_path.is_file() {
            let file_size = std::fs::metadata(&entry_path)
                .map_err(|e| crate::error::SunbeamError::Other(format!("stat: {e}")))?
                .len();
            jobs.push(UploadJob {
                local_path: entry_path,
                parent_id: folder_id.clone(),
                file_size,
                relative_path: format!("{display_prefix}/{name}"),
            });
        }
    }

    Ok(())
}

/// Upload a single file to Drive, updating the progress bar.
async fn upload_single_file_with_progress(
    drive: &super::DriveClient,
    job: &UploadJob,
    pb: &indicatif::ProgressBar,
) -> Result<()> {
    let filename = job
        .local_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unnamed");

    // Create the file item in Drive
    let item = drive
        .create_child(
            &job.parent_id,
            &serde_json::json!({
                "title": filename,
                "filename": filename,
                "type": "file",
            }),
        )
        .await?;

    let item_id = item["id"]
        .as_str()
        .ok_or_else(|| crate::error::SunbeamError::Other("No item ID in response".into()))?;

    let upload_url = item["policy"]
        .as_str()
        .ok_or_else(|| {
            crate::error::SunbeamError::Other(
                "No upload policy URL in response \u{2014} is the item a file?".into(),
            )
        })?;

    tracing::debug!("S3 presigned URL: {upload_url}");

    // Read the file and upload to S3
    let data = std::fs::read(&job.local_path)
        .map_err(|e| crate::error::SunbeamError::Other(format!("reading file: {e}")))?;
    let len = data.len() as u64;
    drive
        .upload_to_s3(upload_url, bytes::Bytes::from(data))
        .await?;
    pb.set_position(len);

    // Notify Drive the upload is complete
    drive.upload_ended(item_id).await?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════
// Mail (Messages)
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum MailCommand {
    /// Mailbox management.
    Mailbox {
        #[command(subcommand)]
        action: MailboxAction,
    },
    /// Message management.
    Message {
        #[command(subcommand)]
        action: MessageAction,
    },
    /// Folder listing.
    Folder {
        #[command(subcommand)]
        action: MailFolderAction,
    },
    /// Contact listing.
    Contact {
        #[command(subcommand)]
        action: MailContactAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum MailboxAction {
    /// List mailboxes.
    List,
    /// Get a mailbox by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MessageAction {
    /// List messages in a mailbox folder.
    List {
        /// Mailbox ID.
        #[arg(short, long)]
        id: String,
        /// Folder name (e.g. "inbox").
        #[arg(short, long, default_value = "inbox")]
        folder: String,
    },
    /// Get a message.
    Get {
        /// Mailbox ID.
        #[arg(short, long)]
        id: String,
        /// Message ID.
        #[arg(short, long)]
        message_id: String,
    },
    /// Send a message from a mailbox.
    Send {
        /// Mailbox ID.
        #[arg(short, long)]
        id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum MailFolderAction {
    /// List folders in a mailbox.
    List {
        /// Mailbox ID.
        #[arg(short, long)]
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum MailContactAction {
    /// List contacts in a mailbox.
    List {
        /// Mailbox ID.
        #[arg(short, long)]
        id: String,
    },
}

pub async fn dispatch_mail(
    cmd: MailCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let mail = client.messages().await?;
    match cmd {
        MailCommand::Mailbox { action } => match action {
            MailboxAction::List => {
                let page_data = mail.list_mailboxes().await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "EMAIL", "DISPLAY_NAME"],
                    |m| {
                        vec![
                            m.id.clone(),
                            m.email.clone().unwrap_or_default(),
                            m.display_name.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            MailboxAction::Get { id } => {
                let item = mail.get_mailbox(&id).await?;
                output::render(&item, fmt)
            }
        },
        MailCommand::Message { action } => match action {
            MessageAction::List { id, folder } => {
                let page_data = mail.list_messages(&id, &folder).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "SUBJECT", "FROM", "READ", "CREATED"],
                    |m| {
                        vec![
                            m.id.clone(),
                            m.subject.clone().unwrap_or_default(),
                            m.from_address.clone().unwrap_or_default(),
                            m.is_read.map_or("-".into(), |r| r.to_string()),
                            m.created_at.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
            MessageAction::Get { id, message_id } => {
                let item = mail.get_message(&id, &message_id).await?;
                output::render(&item, fmt)
            }
            MessageAction::Send { id, data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = mail.send_message(&id, &json).await?;
                output::render(&item, fmt)
            }
        },
        MailCommand::Folder { action } => match action {
            MailFolderAction::List { id } => {
                let page_data = mail.list_folders(&id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "MESSAGES", "UNREAD"],
                    |f| {
                        vec![
                            f.id.clone(),
                            f.name.clone().unwrap_or_default(),
                            f.message_count.map_or("-".into(), |c| c.to_string()),
                            f.unread_count.map_or("-".into(), |c| c.to_string()),
                        ]
                    },
                    fmt,
                )
            }
        },
        MailCommand::Contact { action } => match action {
            MailContactAction::List { id } => {
                let page_data = mail.list_contacts(&id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "EMAIL", "DISPLAY_NAME"],
                    |c| {
                        vec![
                            c.id.clone(),
                            c.email.clone().unwrap_or_default(),
                            c.display_name.clone().unwrap_or_default(),
                        ]
                    },
                    fmt,
                )
            }
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Calendars
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum CalCommand {
    /// Calendar management.
    Calendar {
        #[command(subcommand)]
        action: CalendarAction,
    },
    /// Event management.
    Event {
        #[command(subcommand)]
        action: EventAction,
    },
    /// RSVP to an event.
    Rsvp {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
        /// Event ID.
        #[arg(short = 'e', long)]
        event_id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum CalendarAction {
    /// List calendars.
    List,
    /// Get a calendar by ID.
    Get {
        #[arg(short, long)]
        id: String,
    },
    /// Create a calendar from JSON.
    Create {
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum EventAction {
    /// List events in a calendar.
    List {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
    },
    /// Get an event.
    Get {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
        /// Event ID.
        #[arg(short = 'e', long)]
        event_id: String,
    },
    /// Create an event from JSON.
    Create {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Update an event from JSON.
    Update {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
        /// Event ID.
        #[arg(short = 'e', long)]
        event_id: String,
        /// JSON body (or "-" to read from stdin).
        #[arg(short, long)]
        data: Option<String>,
    },
    /// Delete an event.
    Delete {
        /// Calendar ID.
        #[arg(short = 'c', long)]
        calendar_id: String,
        /// Event ID.
        #[arg(short = 'e', long)]
        event_id: String,
    },
}

pub async fn dispatch_cal(
    cmd: CalCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let cal = client.calendars().await?;
    match cmd {
        CalCommand::Calendar { action } => match action {
            CalendarAction::List => {
                let page_data = cal.list_calendars().await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "NAME", "COLOR", "DEFAULT"],
                    |c| {
                        vec![
                            c.id.clone(),
                            c.name.clone().unwrap_or_default(),
                            c.color.clone().unwrap_or_default(),
                            c.is_default.map_or("-".into(), |d| d.to_string()),
                        ]
                    },
                    fmt,
                )
            }
            CalendarAction::Get { id } => {
                let item = cal.get_calendar(&id).await?;
                output::render(&item, fmt)
            }
            CalendarAction::Create { data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = cal.create_calendar(&json).await?;
                output::render(&item, fmt)
            }
        },
        CalCommand::Event { action } => match action {
            EventAction::List { calendar_id } => {
                let page_data = cal.list_events(&calendar_id).await?;
                output::render_list(
                    &page_data.results,
                    &["ID", "TITLE", "START", "END", "ALL_DAY"],
                    |e| {
                        vec![
                            e.id.clone(),
                            e.title.clone().unwrap_or_default(),
                            e.start.clone().unwrap_or_default(),
                            e.end.clone().unwrap_or_default(),
                            e.all_day.map_or("-".into(), |a| a.to_string()),
                        ]
                    },
                    fmt,
                )
            }
            EventAction::Get {
                calendar_id,
                event_id,
            } => {
                let item = cal.get_event(&calendar_id, &event_id).await?;
                output::render(&item, fmt)
            }
            EventAction::Create { calendar_id, data } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = cal.create_event(&calendar_id, &json).await?;
                output::render(&item, fmt)
            }
            EventAction::Update {
                calendar_id,
                event_id,
                data,
            } => {
                let json = output::read_json_input(data.as_deref())?;
                let item = cal.update_event(&calendar_id, &event_id, &json).await?;
                output::render(&item, fmt)
            }
            EventAction::Delete {
                calendar_id,
                event_id,
            } => {
                cal.delete_event(&calendar_id, &event_id).await?;
                output::ok(&format!("Deleted event {event_id}"));
                Ok(())
            }
        },
        CalCommand::Rsvp {
            calendar_id,
            event_id,
            data,
        } => {
            let json = output::read_json_input(data.as_deref())?;
            cal.rsvp(&calendar_id, &event_id, &json).await?;
            output::ok(&format!("RSVP sent for event {event_id}"));
            Ok(())
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Find
// ═══════════════════════════════════════════════════════════════════════════

#[derive(Subcommand, Debug)]
pub enum FindCommand {
    /// Search across La Suite services.
    Search {
        /// Search query.
        #[arg(short, long)]
        query: String,
        #[arg(long)]
        page: Option<u32>,
    },
}

pub async fn dispatch_find(
    cmd: FindCommand,
    client: &SunbeamClient,
    fmt: OutputFormat,
) -> Result<()> {
    let find = client.find().await?;
    match cmd {
        FindCommand::Search { query, page } => {
            let page_data = find.search(&query, page).await?;
            output::render_list(
                &page_data.results,
                &["ID", "TITLE", "SOURCE", "SCORE", "URL"],
                |r| {
                    vec![
                        r.id.clone(),
                        r.title.clone().unwrap_or_default(),
                        r.source.clone().unwrap_or_default(),
                        r.score.map_or("-".into(), |s| format!("{s:.2}")),
                        r.url.clone().unwrap_or_default(),
                    ]
                },
                fmt,
            )
        }
    }
}
