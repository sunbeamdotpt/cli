//! Per-service image build functions.

use crate::error::{Result, SunbeamError};
use crate::output::step;

use super::{build_image, deploy_rollout, get_build_env};

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

// TODO: first deploy requires registration enabled on tuwunel to create
// the @sol:sunbeam.pt bot account.
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

