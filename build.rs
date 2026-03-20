use flate2::read::GzDecoder;
use std::env;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use tar::Archive;

const KUSTOMIZE_VERSION: &str = "v5.8.1";
const HELM_VERSION: &str = "v4.1.0";

fn main() {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target = env::var("TARGET").unwrap_or_default();
    let (os, arch) = parse_target(&target);

    download_and_embed("kustomize", KUSTOMIZE_VERSION, &os, &arch, &out_dir);
    download_and_embed("helm", HELM_VERSION, &os, &arch, &out_dir);

    // Set version info from git
    let commit = git_commit_sha();
    println!("cargo:rustc-env=SUNBEAM_COMMIT={commit}");

    // Rebuild if git HEAD changes
    println!("cargo:rerun-if-changed=.git/HEAD");
}

fn parse_target(target: &str) -> (String, String) {
    let os = if target.contains("darwin") {
        "darwin"
    } else if target.contains("linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };

    let arch = if target.contains("aarch64") || target.contains("arm64") {
        "arm64"
    } else if target.contains("x86_64") || target.contains("amd64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "amd64"
    };

    (os.to_string(), arch.to_string())
}

fn download_and_embed(tool: &str, version: &str, os: &str, arch: &str, out_dir: &PathBuf) {
    let dest = out_dir.join(tool);
    if dest.exists() {
        return;
    }

    let url = match tool {
        "kustomize" => format!(
            "https://github.com/kubernetes-sigs/kustomize/releases/download/\
             kustomize%2F{version}/kustomize_{version}_{os}_{arch}.tar.gz"
        ),
        "helm" => format!(
            "https://get.helm.sh/helm-{version}-{os}-{arch}.tar.gz"
        ),
        _ => panic!("Unknown tool: {tool}"),
    };

    let extract_path = match tool {
        "kustomize" => "kustomize".to_string(),
        "helm" => format!("{os}-{arch}/helm"),
        _ => unreachable!(),
    };

    eprintln!("cargo:warning=Downloading {tool} {version} for {os}/{arch}...");

    let response = reqwest::blocking::get(&url)
        .unwrap_or_else(|e| panic!("Failed to download {tool}: {e}"));
    let bytes = response
        .bytes()
        .unwrap_or_else(|e| panic!("Failed to read {tool} response: {e}"));

    let decoder = GzDecoder::new(&bytes[..]);
    let mut archive = Archive::new(decoder);

    for entry in archive.entries().expect("Failed to read tar entries") {
        let mut entry = entry.expect("Failed to read tar entry");
        let path = entry
            .path()
            .expect("Failed to read entry path")
            .to_path_buf();
        if path.to_string_lossy() == extract_path {
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .expect("Failed to read binary");
            fs::write(&dest, &data).expect("Failed to write binary");

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))
                    .expect("Failed to set permissions");
            }

            eprintln!("cargo:warning=Embedded {tool} ({} bytes)", data.len());
            return;
        }
    }

    panic!("Could not find {extract_path} in {tool} archive");
}

fn git_commit_sha() -> String {
    Command::new("git")
        .args(["rev-parse", "--short=8", "HEAD"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                Some(String::from_utf8_lossy(&o.stdout).trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}
