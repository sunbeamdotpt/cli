"""Output helpers — step/ok/warn/die + aligned text table."""
import sys


def step(msg: str) -> None:
    """Print a step header."""
    print(f"\n==> {msg}", flush=True)


def ok(msg: str) -> None:
    """Print a success/info line."""
    print(f"    {msg}", flush=True)


def warn(msg: str) -> None:
    """Print a warning to stderr."""
    print(f"    WARN: {msg}", file=sys.stderr, flush=True)


def die(msg: str) -> None:
    """Print an error to stderr and exit."""
    print(f"\nERROR: {msg}", file=sys.stderr, flush=True)
    sys.exit(1)


def table(rows: list[list[str]], headers: list[str]) -> str:
    """Return an aligned text table. Columns padded to max width."""
    if not headers:
        return ""
    # Compute column widths
    col_widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            if i < len(col_widths):
                col_widths[i] = max(col_widths[i], len(cell))
    # Format header
    header_line = "  ".join(h.ljust(col_widths[i]) for i, h in enumerate(headers))
    separator = "  ".join("-" * w for w in col_widths)
    lines = [header_line, separator]
    # Format rows
    for row in rows:
        cells = []
        for i in range(len(headers)):
            val = row[i] if i < len(row) else ""
            cells.append(val.ljust(col_widths[i]))
        lines.append("  ".join(cells))
    return "\n".join(lines)
