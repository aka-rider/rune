#!/usr/bin/env python3
"""
Report Claude Code token usage and USD cost from local session logs.

Usage:
  claude_cost.py --project rune --since 2026-07-25 --until 2026-08-09
  claude_cost.py --group-by day --project rune
"""

import argparse
import csv
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, Iterator, List, Optional, Set, Tuple

PRICES: Dict[str, Dict[str, float]] = {
    "claude-fable-5": {"input": 10.00, "output": 50.00},
    "claude-mythos-5": {"input": 10.00, "output": 50.00},
    "claude-opus-5": {"input": 5.00, "output": 25.00},
    "claude-opus-4-8": {"input": 5.00, "output": 25.00},
    "claude-opus-4-7": {"input": 5.00, "output": 25.00},
    "claude-opus-4-6": {"input": 5.00, "output": 25.00},
    "claude-sonnet-5": {"input": 3.00, "output": 15.00},
    "claude-sonnet-4-6": {"input": 3.00, "output": 15.00},
    "claude-haiku-4-5": {"input": 1.00, "output": 5.00},
    "fast": {"input": 0.0, "output": 0.0},
    "coder": {"input": 0.0, "output": 0.0},
    "qwen3.6-coder": {"input": 0.0, "output": 0.0},
    "<synthetic>": {"input": 0.0, "output": 0.0},
}

SONNET_INTRO = {"claude-sonnet-5": {"input": 2.00, "output": 10.00}}
FAST_MODE_RATE = {"input": 10.00, "output": 50.00}

CACHE_WRITE_5M_MULT = 1.25
CACHE_WRITE_1H_MULT = 2.00
CACHE_READ_MULT = 0.10

COLUMNS = ["group", "input", "cache_w5m", "cache_w1h", "cache_read", "output", "total_tokens", "cost_usd"]


@dataclass
class Totals:
    input: int = 0
    write_5m: int = 0
    write_1h: int = 0
    cache_read: int = 0
    output: int = 0

    def add(self, other: "Totals") -> None:
        self.input += other.input
        self.write_5m += other.write_5m
        self.write_1h += other.write_1h
        self.cache_read += other.cache_read
        self.output += other.output

    def total_tokens(self) -> int:
        return self.input + self.write_5m + self.write_1h + self.cache_read + self.output


@dataclass
class ScanStats:
    files: int = 0
    raw_lines: int = 0
    deduped: int = 0
    malformed: int = 0
    date_min: Optional[str] = None
    date_max: Optional[str] = None


def iter_usage_records(root: Path, stats: ScanStats) -> Iterator[Dict[str, Any]]:
    for session_file in sorted(root.glob("*/*.jsonl")):
        stats.files += 1
        with open(session_file, errors="replace") as handle:
            for line in handle:
                if not line.strip():
                    continue
                try:
                    record = json.loads(line)
                except json.JSONDecodeError:
                    stats.malformed += 1
                    continue
                if record.get("type") != "assistant":
                    continue
                message = record.get("message")
                if not isinstance(message, dict) or not message.get("usage"):
                    continue
                record["_file"] = str(session_file)
                yield record


def dedupe(records: Iterator[Dict[str, Any]], stats: ScanStats) -> Iterator[Dict[str, Any]]:
    seen: Set[Tuple] = set()
    for record in records:
        stats.raw_lines += 1
        message_id = record["message"].get("id")
        if message_id:
            key = (record["_file"], message_id)
        else:
            key = (record["_file"], record.get("requestId"), record.get("uuid"))
        if key in seen:
            continue
        seen.add(key)
        stats.deduped += 1
        yield record


def matches_filters(
    record: Dict[str, Any], since: Optional[str], until: Optional[str], projects: Optional[List[str]]
) -> bool:
    date = record.get("timestamp", "")[:10]
    if since and date < since:
        return False
    if until and date > until:
        return False
    if projects:
        cwd = record.get("cwd", "").lower()
        return any(p.lower() in cwd for p in projects)
    return True


def read_totals(usage: Dict[str, Any]) -> Totals:
    creation = usage.get("cache_creation") or {}
    write_5m = creation.get("ephemeral_5m_input_tokens", 0)
    write_1h = creation.get("ephemeral_1h_input_tokens", 0)
    if not write_5m and not write_1h:
        write_5m = usage.get("cache_creation_input_tokens", 0)
    return Totals(
        input=usage.get("input_tokens", 0),
        write_5m=write_5m,
        write_1h=write_1h,
        cache_read=usage.get("cache_read_input_tokens", 0),
        output=usage.get("output_tokens", 0),
    )


def group_key_for(record: Dict[str, Any], group_by: str) -> Any:
    cwd = record.get("cwd", "unknown")
    model = record["message"].get("model", "unknown")
    if group_by == "project":
        return cwd
    if group_by == "model":
        return model
    if group_by == "day":
        return record.get("timestamp", "unknown")[:10] or "unknown"
    if group_by == "session":
        return record.get("sessionId", "unknown")
    return (cwd, model)


def aggregate(
    records: Iterator[Dict[str, Any]], group_by: str, stats: ScanStats
) -> Tuple[Dict[Any, Dict[Tuple[str, str], Totals]], Set[str]]:
    """Every group keeps per-(model, speed) totals so cost never uses one model's rate for a mixed group."""
    buckets: Dict[Any, Dict[Tuple[str, str], Totals]] = {}
    models: Set[str] = set()

    for record in records:
        message = record["message"]
        model = message.get("model", "unknown")
        usage = message["usage"]
        speed = usage.get("speed") or message.get("speed") or "standard"
        models.add(model)

        date = record.get("timestamp", "")[:10]
        if date:
            if stats.date_min is None or date < stats.date_min:
                stats.date_min = date
            if stats.date_max is None or date > stats.date_max:
                stats.date_max = date

        bucket = buckets.setdefault(group_key_for(record, group_by), {})
        bucket.setdefault((model, speed), Totals()).add(read_totals(usage))

    return buckets, models


def rate_for(model: str, speed: str, prices: Dict[str, Dict[str, float]]) -> Dict[str, float]:
    if speed == "fast" and model in prices and prices[model]["input"] > 0:
        return FAST_MODE_RATE
    return prices.get(model, {"input": 0.0, "output": 0.0})


def cost_for(totals: Totals, model: str, speed: str, prices: Dict[str, Dict[str, float]]) -> float:
    rate = rate_for(model, speed, prices)
    input_rate = rate["input"]
    return (
        totals.input * input_rate
        + totals.write_5m * CACHE_WRITE_5M_MULT * input_rate
        + totals.write_1h * CACHE_WRITE_1H_MULT * input_rate
        + totals.cache_read * CACHE_READ_MULT * input_rate
        + totals.output * rate["output"]
    ) / 1_000_000


def bucket_totals(bucket: Dict[Tuple[str, str], Totals]) -> Totals:
    combined = Totals()
    for totals in bucket.values():
        combined.add(totals)
    return combined


def bucket_cost(bucket: Dict[Tuple[str, str], Totals], prices: Dict[str, Dict[str, float]]) -> float:
    return sum(cost_for(totals, model, speed, prices) for (model, speed), totals in bucket.items())


def sorted_rows(
    buckets: Dict[Any, Dict[Tuple[str, str], Totals]], prices: Dict[str, Dict[str, float]]
) -> List[Tuple[Any, Totals, float]]:
    rows = [(key, bucket_totals(bucket), bucket_cost(bucket, prices)) for key, bucket in buckets.items()]
    rows.sort(key=lambda row: (row[2], row[1].total_tokens()), reverse=True)
    return rows


HEADER = "{:<52} {:>13} {:>13} {:>13} {:>13} {:>13} {:>15} {:>13}"
ROW = "{:<52} {:>13,} {:>13,} {:>13,} {:>13,} {:>13,} {:>15,} {:>13}"


def elide(label: str, width: int = 52) -> str:
    return label if len(label) <= width else "…" + label[-(width - 1):]


def print_row(label: str, totals: Totals, cost: float) -> None:
    print(
        ROW.format(
            elide(label),
            totals.input,
            totals.write_5m,
            totals.write_1h,
            totals.cache_read,
            totals.output,
            totals.total_tokens(),
            f"${cost:,.2f}",
        )
    )


def render_table(
    buckets: Dict[Any, Dict[Tuple[str, str], Totals]],
    prices: Dict[str, Dict[str, float]],
    group_by: str,
    models: Set[str],
    stats: ScanStats,
) -> None:
    print(HEADER.format("Group", "Input", "Cache5m", "Cache1h", "CacheRead", "Output", "TotalTokens", "Cost USD"))
    print("-" * 152)

    grand = Totals()
    grand_cost = 0.0

    if group_by == "project,model":
        by_project: Dict[str, Dict[str, Dict[Tuple[str, str], Totals]]] = {}
        for (project, model), bucket in buckets.items():
            by_project.setdefault(project, {})[model] = bucket

        project_order = sorted(
            by_project.items(),
            key=lambda item: sum(bucket_cost(b, prices) for b in item[1].values()),
            reverse=True,
        )
        for project, models_map in project_order:
            print(f"\n{project}")
            subtotal = Totals()
            subtotal_cost = 0.0
            model_rows = sorted(
                ((model, bucket_totals(b), bucket_cost(b, prices)) for model, b in models_map.items()),
                key=lambda row: (row[2], row[1].total_tokens()),
                reverse=True,
            )
            for model, totals, cost in model_rows:
                print_row(f"  {model}", totals, cost)
                subtotal.add(totals)
                subtotal_cost += cost
            print_row("  SUBTOTAL", subtotal, subtotal_cost)
            grand.add(subtotal)
            grand_cost += subtotal_cost
        print("\n" + "=" * 152)
    else:
        for key, totals, cost in sorted_rows(buckets, prices):
            print_row(str(key), totals, cost)
            grand.add(totals)
            grand_cost += cost
        print("=" * 152)

    print_row("GRAND TOTAL", grand, grand_cost)
    print()
    print_footer(models, prices, stats)


def print_footer(models: Set[str], prices: Dict[str, Dict[str, float]], stats: ScanStats) -> None:
    span = f"{stats.date_min} .. {stats.date_max}" if stats.date_min else "no matching records"
    print(f"Date range covered: {span}")
    print(
        f"Scanned {stats.files:,} session files, {stats.raw_lines:,} raw usage lines "
        f"-> {stats.deduped:,} distinct messages; {stats.malformed:,} malformed lines skipped"
    )
    unpriced = sorted(m for m in models if m not in prices)
    if unpriced:
        print(f"Unpriced models (billed as $0): {', '.join(unpriced)}")
    print("Cache writes billed at 1.25x (5m) / 2.0x (1h) and cache reads at 0.1x the input rate.")


def render_json(
    buckets: Dict[Any, Dict[Tuple[str, str], Totals]],
    prices: Dict[str, Dict[str, float]],
    models: Set[str],
    stats: ScanStats,
) -> None:
    rows = sorted_rows(buckets, prices)
    grand = Totals()
    grand_cost = 0.0
    payload_rows = []
    for key, totals, cost in rows:
        grand.add(totals)
        grand_cost += cost
        payload_rows.append(
            {
                "group": list(key) if isinstance(key, tuple) else key,
                "input": totals.input,
                "cache_w5m": totals.write_5m,
                "cache_w1h": totals.write_1h,
                "cache_read": totals.cache_read,
                "output": totals.output,
                "total_tokens": totals.total_tokens(),
                "cost_usd": round(cost, 2),
            }
        )

    print(
        json.dumps(
            {
                "groups": payload_rows,
                "totals": {
                    "input": grand.input,
                    "cache_w5m": grand.write_5m,
                    "cache_w1h": grand.write_1h,
                    "cache_read": grand.cache_read,
                    "output": grand.output,
                    "total_tokens": grand.total_tokens(),
                    "cost_usd": round(grand_cost, 2),
                },
                "meta": {
                    "date_min": stats.date_min,
                    "date_max": stats.date_max,
                    "files_scanned": stats.files,
                    "raw_usage_lines": stats.raw_lines,
                    "deduped_messages": stats.deduped,
                    "malformed_lines": stats.malformed,
                    "unpriced_models": sorted(m for m in models if m not in prices),
                },
            },
            indent=2,
        )
    )


def render_csv(buckets: Dict[Any, Dict[Tuple[str, str], Totals]], prices: Dict[str, Dict[str, float]]) -> None:
    writer = csv.writer(sys.stdout)
    writer.writerow(COLUMNS)
    for key, totals, cost in sorted_rows(buckets, prices):
        label = " | ".join(key) if isinstance(key, tuple) else str(key)
        writer.writerow(
            [
                label,
                totals.input,
                totals.write_5m,
                totals.write_1h,
                totals.cache_read,
                totals.output,
                totals.total_tokens(),
                f"{cost:.2f}",
            ]
        )


def git_date_range() -> Optional[Tuple[str, str]]:
    try:
        result = subprocess.run(
            ["git", "log", "--format=%ad", "--date=short"], capture_output=True, text=True, check=True
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        print(f"warning: --git-range failed ({exc}); continuing without date bounds", file=sys.stderr)
        return None
    dates = [line.strip() for line in result.stdout.splitlines() if line.strip()]
    return (min(dates), max(dates)) if dates else None


def main() -> None:
    parser = argparse.ArgumentParser(description="Claude Code session cost reporter")
    parser.add_argument("--since", help="inclusive start date, YYYY-MM-DD")
    parser.add_argument("--until", help="inclusive end date, YYYY-MM-DD")
    parser.add_argument("--project", action="append", help="keep records whose cwd contains this substring (repeatable)")
    parser.add_argument(
        "--group-by",
        default="project,model",
        choices=["project", "model", "project,model", "day", "session"],
    )
    parser.add_argument("--git-range", action="store_true", help="default the date range from git log in the cwd")
    parser.add_argument("--prices", help="JSON file merged over the built-in rate table")
    parser.add_argument("--sonnet-intro", action="store_true", help="price claude-sonnet-5 at introductory $2/$10")
    parser.add_argument("--root", default="~/.claude/projects")
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--csv", action="store_true")
    args = parser.parse_args()

    since, until = args.since, args.until
    if args.git_range:
        span = git_date_range()
        if span:
            since = since or span[0]
            until = until or span[1]

    prices = dict(PRICES)
    if args.sonnet_intro:
        prices.update(SONNET_INTRO)
    if args.prices:
        with open(args.prices) as handle:
            prices.update(json.load(handle))

    root = Path(args.root).expanduser()
    if not root.is_dir():
        sys.exit(f"error: {root} is not a directory")

    stats = ScanStats()
    records = dedupe(iter_usage_records(root, stats), stats)
    kept = (r for r in records if matches_filters(r, since, until, args.project))
    buckets, models = aggregate(kept, args.group_by, stats)

    if args.json:
        render_json(buckets, prices, models, stats)
    elif args.csv:
        render_csv(buckets, prices)
    else:
        render_table(buckets, prices, args.group_by, models, stats)


if __name__ == "__main__":
    main()
