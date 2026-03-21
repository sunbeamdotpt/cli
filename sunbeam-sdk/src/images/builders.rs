//! Per-service image build functions.

use crate::error::{Result, ResultExt, SunbeamError};
use crate::output::{ok, step, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::{build_image, deploy_rollout, get_build_env};

/// Message component definition: (cli_name, image_name, dockerfile_rel, target).
pub const MESSAGES_COMPONENTS: &[(&str, &str, &str, Option<&str>)] = &[
    (
        "messages-backend",
        "messages-backend",
        "src/backend/Dockerfile",
        Some("runtime-distroless-prod"),
    ),
    (
        "messages-frontend",
        "messages-frontend",
        "src/frontend/Dockerfile",
        Some("runtime-prod"),
    ),
    (
        "messages-mta-in",
        "messages-mta-in",
        "src/mta-in/Dockerfile",
        None,
    ),
    (
        "messages-mta-out",
        "messages-mta-out",
        "src/mta-out/Dockerfile",
        None,
    ),
    (
        "messages-mpa",
        "messages-mpa",
        "src/mpa/rspamd/Dockerfile",
        None,
    ),
    (
        "messages-socks-proxy",
        "messages-socks-proxy",
        "src/socks-proxy/Dockerfile",
        None,
    ),
];

pub async fn build_proxy(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let proxy_dir = crate::config::get_repo_root().join("proxy");
    if !proxy_dir.is_dir() {
        return Err(SunbeamError::build(format!("Proxy source not found at {}", proxy_dir.display())));
    }

    let image = format!("{}/studio/proxy:latest", env.registry);
    step(&format!("Building sunbeam-proxy -> {image} ..."));

    build_image(
        &env,
        &image,
        &proxy_dir.join("Dockerfile"),
        &proxy_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["pingora"], "ingress", 120, Some(&[image])).await?;
    }
    Ok(())
}

pub async fn build_tuwunel(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let tuwunel_dir = crate::config::get_repo_root().join("tuwunel");
    if !tuwunel_dir.is_dir() {
        return Err(SunbeamError::build(format!("Tuwunel source not found at {}", tuwunel_dir.display())));
    }

    let image = format!("{}/studio/tuwunel:latest", env.registry);
    step(&format!("Building tuwunel -> {image} ..."));

    build_image(
        &env,
        &image,
        &tuwunel_dir.join("Dockerfile"),
        &tuwunel_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["tuwunel"], "matrix", 180, Some(&[image])).await?;
    }
    Ok(())
}

pub async fn build_integration(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let sunbeam_dir = crate::config::get_repo_root();
    let integration_service_dir = sunbeam_dir.join("integration-service");
    let dockerfile = integration_service_dir.join("Dockerfile");
    let dockerignore = integration_service_dir.join(".dockerignore");

    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!(
            "integration-service Dockerfile not found at {}",
            dockerfile.display()
        )));
    }
    if !sunbeam_dir
        .join("integration")
        .join("packages")
        .join("widgets")
        .is_dir()
    {
        return Err(SunbeamError::build(format!(
            "integration repo not found at {} -- \
             run: cd sunbeam && git clone https://github.com/suitenumerique/integration.git",
            sunbeam_dir.join("integration").display()
        )));
    }

    let image = format!("{}/studio/integration:latest", env.registry);
    step(&format!("Building integration -> {image} ..."));

    // .dockerignore needs to be at context root
    let root_ignore = sunbeam_dir.join(".dockerignore");
    let mut copied_ignore = false;
    if !root_ignore.exists() && dockerignore.exists() {
        std::fs::copy(&dockerignore, &root_ignore).ok();
        copied_ignore = true;
    }

    let result = build_image(
        &env,
        &image,
        &dockerfile,
        &sunbeam_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await;

    if copied_ignore && root_ignore.exists() {
        let _ = std::fs::remove_file(&root_ignore);
    }

    result?;

    if deploy {
        deploy_rollout(&env, &["integration"], "lasuite", 120, None).await?;
    }
    Ok(())
}

pub async fn build_kratos_admin(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let kratos_admin_dir = crate::config::get_repo_root().join("kratos-admin");
    if !kratos_admin_dir.is_dir() {
        return Err(SunbeamError::build(format!(
            "kratos-admin source not found at {}",
            kratos_admin_dir.display()
        )));
    }

    let image = format!("{}/studio/kratos-admin-ui:latest", env.registry);
    step(&format!("Building kratos-admin-ui -> {image} ..."));

    build_image(
        &env,
        &image,
        &kratos_admin_dir.join("Dockerfile"),
        &kratos_admin_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["kratos-admin-ui"], "ory", 120, None).await?;
    }
    Ok(())
}

pub async fn build_meet(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let meet_dir = crate::config::get_repo_root().join("meet");
    if !meet_dir.is_dir() {
        return Err(SunbeamError::build(format!("meet source not found at {}", meet_dir.display())));
    }

    let backend_image = format!("{}/studio/meet-backend:latest", env.registry);
    let frontend_image = format!("{}/studio/meet-frontend:latest", env.registry);

    // Backend
    step(&format!("Building meet-backend -> {backend_image} ..."));
    build_image(
        &env,
        &backend_image,
        &meet_dir.join("Dockerfile"),
        &meet_dir,
        Some("backend-production"),
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    // Frontend
    step(&format!("Building meet-frontend -> {frontend_image} ..."));
    let frontend_dockerfile = meet_dir.join("src").join("frontend").join("Dockerfile");
    if !frontend_dockerfile.exists() {
        return Err(SunbeamError::build(format!(
            "meet frontend Dockerfile not found at {}",
            frontend_dockerfile.display()
        )));
    }

    let mut build_args = HashMap::new();
    build_args.insert("VITE_API_BASE_URL".to_string(), String::new());

    build_image(
        &env,
        &frontend_image,
        &frontend_dockerfile,
        &meet_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(
            &env,
            &["meet-backend", "meet-celery-worker", "meet-frontend"],
            "lasuite",
            180,
            None,
        )
        .await?;
    }
    Ok(())
}

pub async fn build_people(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let people_dir = crate::config::get_repo_root().join("people");
    if !people_dir.is_dir() {
        return Err(SunbeamError::build(format!("people source not found at {}", people_dir.display())));
    }

    let workspace_dir = people_dir.join("src").join("frontend");
    let app_dir = workspace_dir.join("apps").join("desk");
    let dockerfile = workspace_dir.join("Dockerfile");
    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!("Dockerfile not found at {}", dockerfile.display())));
    }

    let image = format!("{}/studio/people-frontend:latest", env.registry);
    step(&format!("Building people-frontend -> {image} ..."));

    // yarn install
    ok("Updating yarn.lock (yarn install in workspace)...");
    let yarn_status = tokio::process::Command::new("yarn")
        .args(["install", "--ignore-engines"])
        .current_dir(&workspace_dir)
        .status()
        .await
        .ctx("Failed to run yarn install")?;
    if !yarn_status.success() {
        return Err(SunbeamError::tool("yarn", "install failed"));
    }

    // cunningham design tokens
    ok("Regenerating cunningham design tokens...");
    let cunningham_bin = workspace_dir
        .join("node_modules")
        .join(".bin")
        .join("cunningham");
    let cunningham_status = tokio::process::Command::new(&cunningham_bin)
        .args(["-g", "css,ts", "-o", "src/cunningham", "--utility-classes"])
        .current_dir(&app_dir)
        .status()
        .await
        .ctx("Failed to run cunningham")?;
    if !cunningham_status.success() {
        return Err(SunbeamError::tool("cunningham", "design token generation failed"));
    }

    let mut build_args = HashMap::new();
    build_args.insert("DOCKER_USER".to_string(), "101".to_string());

    build_image(
        &env,
        &image,
        &dockerfile,
        &people_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["people-frontend"], "lasuite", 180, None).await?;
    }
    Ok(())
}

pub async fn build_messages(what: &str, push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let messages_dir = crate::config::get_repo_root().join("messages");
    if !messages_dir.is_dir() {
        return Err(SunbeamError::build(format!("messages source not found at {}", messages_dir.display())));
    }

    let components: Vec<_> = if what == "messages" {
        MESSAGES_COMPONENTS.to_vec()
    } else {
        MESSAGES_COMPONENTS
            .iter()
            .filter(|(name, _, _, _)| *name == what)
            .copied()
            .collect()
    };

    let mut built_images = Vec::new();

    for (component, image_name, dockerfile_rel, target) in &components {
        let dockerfile = messages_dir.join(dockerfile_rel);
        if !dockerfile.exists() {
            warn(&format!(
                "Dockerfile not found at {} -- skipping {component}",
                dockerfile.display()
            ));
            continue;
        }

        let image = format!("{}/studio/{image_name}:latest", env.registry);
        let context_dir = dockerfile.parent().unwrap_or(&messages_dir);
        step(&format!("Building {component} -> {image} ..."));

        // Patch ghcr.io/astral-sh/uv COPY for messages-backend on local builds
        let mut cleanup_paths = Vec::new();
        let actual_dockerfile;

        if !env.is_prod && *image_name == "messages-backend" {
            let (patched, cleanup) =
                patch_dockerfile_uv(&dockerfile, context_dir, &env.platform).await?;
            actual_dockerfile = patched;
            cleanup_paths = cleanup;
        } else {
            actual_dockerfile = dockerfile.clone();
        }

        build_image(
            &env,
            &image,
            &actual_dockerfile,
            context_dir,
            *target,
            None,
            push,
            no_cache,
            &cleanup_paths,
        )
        .await?;

        built_images.push(image);
    }

    if deploy && !built_images.is_empty() {
        deploy_rollout(
            &env,
            &[
                "messages-backend",
                "messages-worker",
                "messages-frontend",
                "messages-mta-in",
                "messages-mta-out",
                "messages-mpa",
                "messages-socks-proxy",
            ],
            "lasuite",
            180,
            None,
        )
        .await?;
    }

    Ok(())
}

/// Build a La Suite frontend image from source and push to the Gitea registry.
#[allow(clippy::too_many_arguments)]
pub async fn build_la_suite_frontend(
    app: &str,
    repo_dir: &Path,
    workspace_rel: &str,
    app_rel: &str,
    dockerfile_rel: &str,
    image_name: &str,
    deployment: &str,
    namespace: &str,
    push: bool,
    deploy: bool,
    no_cache: bool,
) -> Result<()> {
    let env = get_build_env().await?;

    let workspace_dir = repo_dir.join(workspace_rel);
    let app_dir = repo_dir.join(app_rel);
    let dockerfile = repo_dir.join(dockerfile_rel);

    if !repo_dir.is_dir() {
        return Err(SunbeamError::build(format!("{app} source not found at {}", repo_dir.display())));
    }
    if !dockerfile.exists() {
        return Err(SunbeamError::build(format!("Dockerfile not found at {}", dockerfile.display())));
    }

    let image = format!("{}/studio/{image_name}:latest", env.registry);
    step(&format!("Building {app} -> {image} ..."));

    ok("Updating yarn.lock (yarn install in workspace)...");
    let yarn_status = tokio::process::Command::new("yarn")
        .args(["install", "--ignore-engines"])
        .current_dir(&workspace_dir)
        .status()
        .await
        .ctx("Failed to run yarn install")?;
    if !yarn_status.success() {
        return Err(SunbeamError::tool("yarn", "install failed"));
    }

    ok("Regenerating cunningham design tokens (yarn build-theme)...");
    let theme_status = tokio::process::Command::new("yarn")
        .args(["build-theme"])
        .current_dir(&app_dir)
        .status()
        .await
        .ctx("Failed to run yarn build-theme")?;
    if !theme_status.success() {
        return Err(SunbeamError::tool("yarn", "build-theme failed"));
    }

    let mut build_args = HashMap::new();
    build_args.insert("DOCKER_USER".to_string(), "101".to_string());

    build_image(
        &env,
        &image,
        &dockerfile,
        repo_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &[deployment], namespace, 180, None).await?;
    }
    Ok(())
}

/// Download uv from GitHub releases and return a patched Dockerfile path.
pub async fn patch_dockerfile_uv(
    dockerfile_path: &Path,
    context_dir: &Path,
    platform: &str,
) -> Result<(PathBuf, Vec<PathBuf>)> {
    let content = std::fs::read_to_string(dockerfile_path)
        .ctx("Failed to read Dockerfile for uv patching")?;

    // Match COPY --from=ghcr.io/astral-sh/uv@sha256:... /uv /uvx /bin/
    let original_copy = content
        .lines()
        .find(|line| {
            line.contains("COPY")
                && line.contains("--from=ghcr.io/astral-sh/uv@sha256:")
                && line.contains("/uv")
                && line.contains("/bin/")
        })
        .map(|line| line.trim().to_string());

    let original_copy = match original_copy {
        Some(c) => c,
        None => return Ok((dockerfile_path.to_path_buf(), vec![])),
    };

    // Find uv version from comment like: oci://ghcr.io/astral-sh/uv:0.x.y
    let version = content
        .lines()
        .find_map(|line| {
            let marker = "oci://ghcr.io/astral-sh/uv:";
            if let Some(idx) = line.find(marker) {
                let rest = &line[idx + marker.len()..];
                let ver = rest.split_whitespace().next().unwrap_or("");
                if !ver.is_empty() {
                    Some(ver.to_string())
                } else {
                    None
                }
            } else {
                None
            }
        });

    let version = match version {
        Some(v) => v,
        None => {
            warn("Could not find uv version comment in Dockerfile; ghcr.io pull may fail.");
            return Ok((dockerfile_path.to_path_buf(), vec![]));
        }
    };

    let arch = if platform.contains("amd64") {
        "x86_64"
    } else {
        "aarch64"
    };

    let url = format!(
        "https://github.com/astral-sh/uv/releases/download/{version}/uv-{arch}-unknown-linux-gnu.tar.gz"
    );

    let stage_dir = context_dir.join("_sunbeam_uv_stage");
    let patched_df = dockerfile_path
        .parent()
        .unwrap_or(dockerfile_path)
        .join("Dockerfile._sunbeam_patched");
    let cleanup = vec![stage_dir.clone(), patched_df.clone()];

    ok(&format!(
        "Downloading uv {version} ({arch}) from GitHub releases to bypass ghcr.io..."
    ));

    std::fs::create_dir_all(&stage_dir)?;

    // Download tarball
    let response = reqwest::get(&url)
        .await
        .ctx("Failed to download uv release")?;
    let tarball_bytes = response.bytes().await?;

    // Extract uv and uvx from tarball
    let decoder = flate2::read::GzDecoder::new(&tarball_bytes[..]);
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        let file_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();

        if (file_name == "uv" || file_name == "uvx") && entry.header().entry_type().is_file() {
            let dest = stage_dir.join(&file_name);
            let mut outfile = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut outfile)?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o755))?;
            }
        }
    }

    if !stage_dir.join("uv").exists() {
        warn("uv binary not found in release tarball; build may fail.");
        return Ok((dockerfile_path.to_path_buf(), cleanup));
    }

    let patched = content.replace(
        &original_copy,
        "COPY _sunbeam_uv_stage/uv _sunbeam_uv_stage/uvx /bin/",
    );
    std::fs::write(&patched_df, patched)?;
    ok(&format!("  uv {version} staged; using patched Dockerfile."));

    Ok((patched_df, cleanup))
}

pub async fn build_projects(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let projects_dir = crate::config::get_repo_root().join("projects");
    if !projects_dir.is_dir() {
        return Err(SunbeamError::build(format!("projects source not found at {}", projects_dir.display())));
    }

    let image = format!("{}/studio/projects:latest", env.registry);
    step(&format!("Building projects -> {image} ..."));

    build_image(
        &env,
        &image,
        &projects_dir.join("Dockerfile"),
        &projects_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["projects"], "lasuite", 180, Some(&[image])).await?;
    }
    Ok(())
}

// TODO: first deploy requires registration enabled on tuwunel to create
// the @sol:sunbeam.pt bot account. Flow:
//   1. Set allow_registration = true in tuwunel-config.yaml
//   2. Apply + restart tuwunel
//   3. Register bot via POST /_matrix/client/v3/register with registration token
//   4. Store access_token + device_id in OpenBao at secret/sol
//   5. Set allow_registration = false, re-apply
//   6. Then build + deploy sol
// This should be automated as `sunbeam user create-bot <name>`.
pub async fn build_sol(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let sol_dir = crate::config::get_repo_root().join("sol");
    if !sol_dir.is_dir() {
        return Err(SunbeamError::build(format!("Sol source not found at {}", sol_dir.display())));
    }

    let image = format!("{}/studio/sol:latest", env.registry);
    step(&format!("Building sol -> {image} ..."));

    build_image(
        &env,
        &image,
        &sol_dir.join("Dockerfile"),
        &sol_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(&env, &["sol"], "matrix", 120, None).await?;
    }
    Ok(())
}

pub async fn build_calendars(push: bool, deploy: bool, no_cache: bool) -> Result<()> {
    let env = get_build_env().await?;
    let cal_dir = crate::config::get_repo_root().join("calendars");
    if !cal_dir.is_dir() {
        return Err(SunbeamError::build(format!("calendars source not found at {}", cal_dir.display())));
    }

    let backend_dir = cal_dir.join("src").join("backend");
    let backend_image = format!("{}/studio/calendars-backend:latest", env.registry);
    step(&format!("Building calendars-backend -> {backend_image} ..."));

    // Stage translations.json into the build context
    let translations_src = cal_dir
        .join("src")
        .join("frontend")
        .join("apps")
        .join("calendars")
        .join("src")
        .join("features")
        .join("i18n")
        .join("translations.json");

    let translations_dst = backend_dir.join("_translations.json");
    let mut cleanup: Vec<PathBuf> = Vec::new();
    let mut dockerfile = backend_dir.join("Dockerfile");

    if translations_src.exists() {
        std::fs::copy(&translations_src, &translations_dst)?;
        cleanup.push(translations_dst);

        // Patch Dockerfile to COPY translations into production image
        let mut content = std::fs::read_to_string(&dockerfile)?;
        content.push_str(
            "\n# Sunbeam: bake translations.json for default calendar names\n\
             COPY _translations.json /data/translations.json\n",
        );
        let patched_df = backend_dir.join("Dockerfile._sunbeam_patched");
        std::fs::write(&patched_df, content)?;
        cleanup.push(patched_df.clone());
        dockerfile = patched_df;
    }

    build_image(
        &env,
        &backend_image,
        &dockerfile,
        &backend_dir,
        Some("backend-production"),
        None,
        push,
        no_cache,
        &cleanup,
    )
    .await?;

    // caldav
    let caldav_image = format!("{}/studio/calendars-caldav:latest", env.registry);
    step(&format!("Building calendars-caldav -> {caldav_image} ..."));
    let caldav_dir = cal_dir.join("src").join("caldav");
    build_image(
        &env,
        &caldav_image,
        &caldav_dir.join("Dockerfile"),
        &caldav_dir,
        None,
        None,
        push,
        no_cache,
        &[],
    )
    .await?;

    // frontend
    let frontend_image = format!("{}/studio/calendars-frontend:latest", env.registry);
    step(&format!(
        "Building calendars-frontend -> {frontend_image} ..."
    ));
    let integration_base = format!("https://integration.{}", env.domain);
    let mut build_args = HashMap::new();
    build_args.insert(
        "VISIO_BASE_URL".to_string(),
        format!("https://meet.{}", env.domain),
    );
    build_args.insert(
        "GAUFRE_WIDGET_PATH".to_string(),
        format!("{integration_base}/api/v2/lagaufre.js"),
    );
    build_args.insert(
        "GAUFRE_API_URL".to_string(),
        format!("{integration_base}/api/v2/services.json"),
    );
    build_args.insert(
        "THEME_CSS_URL".to_string(),
        format!("{integration_base}/api/v2/theme.css"),
    );

    let frontend_dir = cal_dir.join("src").join("frontend");
    build_image(
        &env,
        &frontend_image,
        &frontend_dir.join("Dockerfile"),
        &frontend_dir,
        Some("frontend-production"),
        Some(&build_args),
        push,
        no_cache,
        &[],
    )
    .await?;

    if deploy {
        deploy_rollout(
            &env,
            &[
                "calendars-backend",
                "calendars-worker",
                "calendars-caldav",
                "calendars-frontend",
            ],
            "lasuite",
            180,
            Some(&[backend_image, caldav_image, frontend_image]),
        )
        .await?;
    }
    Ok(())
}
