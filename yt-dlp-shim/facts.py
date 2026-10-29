"""What a resolved audio stream is: its format (itag), container, bit rate and exact size.

yt-dlp knows the size of the format it picked (`filesize`, from the player response's
contentLength) and prints it beside the URL in the same run, so the size costs nothing extra.
When yt-dlp has no size (it prints NA), the `clen` parameter of the googlevideo URL is the
fallback. Kept free of any app.py import so it stays cheap to unit test.
"""
from __future__ import annotations

import re
from typing import Optional
from urllib.parse import parse_qs, urlsplit

# One --print template for every run that selects the format: format id, size, container,
# average bit rate in kbps and the time the URL may first be fetched, separated by bars. yt-dlp
# prints NA for a field it does not know.
FORMAT_PRINT = "%(format_id)s|%(filesize)s|%(ext)s|%(abr)s|%(available_at)s"

# A YouTube video id: eleven letters, digits, dashes or underscores.
_VIDEO_ID = re.compile(r"[A-Za-z0-9_-]{11}")

# Containers by the MIME type googlevideo names in the URL.
_EXT_BY_MIME = {"audio/mp4": "m4a", "audio/webm": "webm", "video/mp4": "mp4", "video/webm": "webm"}


def is_video_id(value: Optional[str]) -> bool:
    """Whether `value` is a YouTube video id, and so safe to hand yt-dlp as one."""
    return bool(value) and _VIDEO_ID.fullmatch(value) is not None


def _text(value: str) -> Optional[str]:
    value = value.strip()
    return None if not value or value.upper() == "NA" or value == "None" else value


def _size(value) -> Optional[int]:
    try:
        size = int(str(value).strip())
    except (TypeError, ValueError):
        return None
    return size if size > 0 else None


def _kbps(value) -> Optional[int]:
    try:
        kbps = round(float(str(value).strip()))
    except (TypeError, ValueError):
        return None
    return kbps if kbps > 0 else None


def is_format_line(line: str) -> bool:
    """Whether a line of yt-dlp output is the FORMAT_PRINT line."""
    return line.count("|") in (3, 4) and not line.startswith("http") and not line.startswith("{")


def parse_format_line(line: str) -> dict:
    """The FORMAT_PRINT line as {itag, size, ext, abr}; a field yt-dlp did not know is None."""
    parts = (line or "").strip().split("|")
    if len(parts) not in (4, 5):
        return {"itag": None, "size": None, "ext": None, "abr": None}
    itag, size, ext, abr = parts[:4]
    return {"itag": _text(itag), "size": _size(size), "ext": _text(ext), "abr": _kbps(abr)}


def available_at(line: Optional[str]) -> Optional[float]:
    """The Unix time the FORMAT_PRINT line says its URL may first be fetched. googlevideo refuses
    the URL with a 403 before then. None when the line does not say."""
    parts = (line or "").strip().split("|")
    if len(parts) != 5:
        return None
    try:
        at = float(parts[4].strip())
    except ValueError:
        return None
    return at if at > 0 else None


def facts_from_url(url: Optional[str]) -> dict:
    """What a googlevideo URL says of itself: itag, clen (the size) and the container."""
    if not url:
        return {"itag": None, "size": None, "ext": None, "abr": None}
    query = parse_qs(urlsplit(url).query)

    def first(name: str) -> Optional[str]:
        values = query.get(name)
        return values[0] if values else None

    mime = (first("mime") or "").split(";")[0].strip().lower()
    return {
        "itag": _text(first("itag") or ""),
        "size": _size(first("clen")),
        "ext": _EXT_BY_MIME.get(mime),
        "abr": None,
    }


def stream_facts(format_line: Optional[str], url: Optional[str]) -> dict:
    """The printed facts, with the URL's own filling what yt-dlp left out."""
    printed = parse_format_line(format_line) if format_line else parse_format_line("")
    from_url = facts_from_url(url)
    return {key: printed[key] if printed[key] is not None else from_url[key] for key in printed}


def split_run(out: str) -> tuple[list[str], Optional[str], Optional[str]]:
    """A run's output lines as (the other lines, the format line, the first URL)."""
    lines = [ln.strip() for ln in (out or "").strip().split("\n") if ln.strip()]
    url = next((ln for ln in lines if ln.startswith("http")), None)
    format_line = next((ln for ln in lines if is_format_line(ln)), None)
    rest = [ln for ln in lines if ln != url and ln != format_line]
    return rest, format_line, url


def parse_range(value: Optional[str]) -> Optional[tuple[Optional[int], Optional[int]]]:
    """One byte range as (first, last): last is None for an open range ("bytes=100-"), first is
    None for a suffix range ("bytes=-500"). None for no header, several ranges or anything else."""
    if not value:
        return None
    text = value.replace(" ", "").lower()
    if not text.startswith("bytes=") or "," in text:
        return None
    first, sep, last = text[len("bytes="):].partition("-")
    if not sep:
        return None
    try:
        start = int(first) if first else None
        end = int(last) if last else None
    except ValueError:
        return None
    if start is None and end is None:
        return None
    if start is not None and end is not None and end < start:
        return None
    return start, end


def range_start(value: Optional[str]) -> Optional[int]:
    """The first byte a Content-Range covers ("bytes 5-9/1234" is 5), or None."""
    if not value:
        return None
    match = re.match(r"\s*bytes\s+(\d+)-\d+/", value)
    return int(match.group(1)) if match else None


def range_end(value: Optional[str]) -> Optional[int]:
    """The last byte a Content-Range covers ("bytes 0-1/1234" is 1), or None."""
    if not value:
        return None
    match = re.match(r"\s*bytes\s+\d+-(\d+)/", value)
    return int(match.group(1)) if match else None


def total_from_content_range(value: Optional[str]) -> Optional[int]:
    """The full length after the slash of a Content-Range ("bytes 0-1/1234", "bytes */1234")."""
    if not value or "/" not in value:
        return None
    return _size(value.rsplit("/", 1)[1])
