"""Configuration management — load/save ~/.sunbeam.json for production host and infra directory."""
import json
import os
from pathlib import Path
from typing import Optional


CONFIG_PATH = Path.home() / ".sunbeam.json"


class SunbeamConfig:
    """Sunbeam configuration with production host and infrastructure directory."""
    
    def __init__(self, production_host: str = "", infra_directory: str = "",
                 acme_email: str = ""):
        self.production_host = production_host
        self.infra_directory = infra_directory
        self.acme_email = acme_email

    def to_dict(self) -> dict:
        """Convert configuration to dictionary for JSON serialization."""
        return {
            "production_host": self.production_host,
            "infra_directory": self.infra_directory,
            "acme_email": self.acme_email,
        }

    @classmethod
    def from_dict(cls, data: dict) -> 'SunbeamConfig':
        """Create configuration from dictionary."""
        return cls(
            production_host=data.get("production_host", ""),
            infra_directory=data.get("infra_directory", ""),
            acme_email=data.get("acme_email", ""),
        )


def load_config() -> SunbeamConfig:
    """Load configuration from ~/.sunbeam.json, return empty config if not found."""
    if not CONFIG_PATH.exists():
        return SunbeamConfig()
    
    try:
        with open(CONFIG_PATH, 'r') as f:
            data = json.load(f)
        return SunbeamConfig.from_dict(data)
    except (json.JSONDecodeError, IOError, KeyError) as e:
        from sunbeam.output import warn
        warn(f"Failed to load config from {CONFIG_PATH}: {e}")
        return SunbeamConfig()


def save_config(config: SunbeamConfig) -> None:
    """Save configuration to ~/.sunbeam.json."""
    try:
        CONFIG_PATH.parent.mkdir(parents=True, exist_ok=True)
        with open(CONFIG_PATH, 'w') as f:
            json.dump(config.to_dict(), f, indent=2)
        from sunbeam.output import ok
        ok(f"Configuration saved to {CONFIG_PATH}")
    except IOError as e:
        from sunbeam.output import die
        die(f"Failed to save config to {CONFIG_PATH}: {e}")


def get_production_host() -> str:
    """Get production host from config or SUNBEAM_SSH_HOST environment variable."""
    config = load_config()
    if config.production_host:
        return config.production_host
    return os.environ.get("SUNBEAM_SSH_HOST", "")


def get_infra_directory() -> str:
    """Get infrastructure directory from config."""
    config = load_config()
    return config.infra_directory


def get_infra_dir() -> "Path":
    """Infrastructure manifests directory as a Path.

    Prefers the configured infra_directory; falls back to the package-relative
    path (works when running from the development checkout).
    """
    from pathlib import Path
    configured = load_config().infra_directory
    if configured:
        return Path(configured)
    # Dev fallback: cli/sunbeam/config.py → parents[0]=cli/sunbeam, [1]=cli, [2]=monorepo root
    return Path(__file__).resolve().parents[2] / "infrastructure"


def get_repo_root() -> "Path":
    """Monorepo root directory (parent of the infrastructure directory)."""
    return get_infra_dir().parent
