"""/stream asks googlevideo for a wide range in pieces and sends them back as one answer."""
import os
import sys

import pytest
import requests

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

app = pytest.importorskip("app")

DATA = bytes(range(256)) * 4  # 1024 bytes, each position recognizable
PIECE = 100


class Piece:
    def __init__(self, status, headers, body, break_after=None):
        self.status_code = status
        self.headers = headers
        self.body = body
        self.break_after = break_after
        self.closed = False

    def iter_content(self, chunk_size):
        sent = 0
        while sent < len(self.body):
            if self.break_after is not None and sent >= self.break_after:
                raise requests.ConnectionError("connection reset")
            step = min(7, len(self.body) - sent)
            if self.break_after is not None:
                step = min(step, self.break_after - sent) or step
            yield self.body[sent:sent + step]
            sent += step

    def close(self):
        self.closed = True


class GoogleVideo:
    """Answers each range exactly, as googlevideo does, and remembers what was asked."""

    def __init__(self, data=DATA):
        self.data = data
        self.asked = []
        self.opened = []
        self.refuse_next_from = None
        self.break_next_from = None
        # Opens of a piece from this byte that fail (no answer at all), how many times.
        self.unreachable_from = {}
        # The piece from this byte answers a plain 200 of the whole file.
        self.whole_from = None
        # Pieces from these bytes answer with no Content-Range.
        self.no_range_from = set()
        # A URL with this suffix serves another file, as a fresh resolve to another format can.
        self.other_file_url = None
        self.other_data = bytes(range(255, -1, -1)) * 8

    def open(self, url, headers, video_id):
        rng = headers["Range"]
        self.asked.append((url, rng))
        data = self.other_data if self.other_file_url and url.endswith(self.other_file_url) else self.data
        spec = rng[len("bytes="):]
        first, _, last = spec.partition("-")
        total = len(data)
        if first == "":
            start, end = total - int(last), total - 1
        else:
            start = int(first)
            end = min(int(last), total - 1) if last else total - 1
        if self.unreachable_from.get(start, 0) > 0:
            self.unreachable_from[start] -= 1
            return None
        if start >= total:
            return Piece(416, {"Content-Range": f"bytes */{total}"}, b"")
        if self.refuse_next_from == start:
            self.refuse_next_from = None
            return Piece(403, {}, b"")
        if self.whole_from == start:
            piece = Piece(200, {"Content-Type": "audio/mp4", "Content-Length": str(total)}, data)
            self.opened.append(piece)
            return piece
        break_after = None
        if self.break_next_from == start:
            self.break_next_from = None
            break_after = 13
        headers = {"Content-Type": "audio/mp4", "Content-Length": str(end - start + 1)}
        if start not in self.no_range_from:
            headers["Content-Range"] = f"bytes {start}-{end}/{total}"
        piece = Piece(206, headers, data[start:end + 1], break_after)
        self.opened.append(piece)
        return piece


@pytest.fixture
def video(monkeypatch):
    upstream = GoogleVideo()
    resolved = []

    def resolve(video_id, bg=False):
        resolved.append(video_id)
        return f"https://googlevideo.example/{len(resolved)}"

    monkeypatch.setattr(app, "_UPSTREAM_PIECE", PIECE)
    monkeypatch.setattr(app, "_PIECE_RETRY_WAIT", 0)
    monkeypatch.setattr(app, "_resolve_url", resolve)
    monkeypatch.setattr(app, "_url_cache_evict_if", lambda video_id, url: None)
    monkeypatch.setattr(app, "_maybe_measure_loudness", lambda video_id, url: False)
    monkeypatch.setattr(app, "_open_upstream", upstream.open)
    upstream.resolved = resolved
    return upstream


def get(range_header=None):
    headers = {"Range": range_header} if range_header else {}
    return app.app.test_client().get("/stream?id=vid-pieces", headers=headers)


def ranges(upstream):
    return [rng for _, rng in upstream.asked]


def test_an_open_range_comes_back_whole_from_pieces(video):
    answer = get("bytes=0-")
    assert answer.status_code == 206
    assert answer.headers["Content-Range"] == "bytes 0-1023/1024"
    assert answer.headers["Content-Length"] == "1024"
    assert answer.headers["Content-Type"] == "audio/mp4"
    assert answer.data == DATA
    assert ranges(video)[:3] == ["bytes=0-99", "bytes=100-199", "bytes=200-299"]
    assert ranges(video)[-1] == "bytes=1000-1023"
    assert app._facts_get("vid-pieces")["size"] == 1024


def test_a_range_past_the_start_like_avplayers_walk_is_pieced_too(video):
    answer = get("bytes=131-")
    assert answer.status_code == 206
    assert answer.headers["Content-Range"] == "bytes 131-1023/1024"
    assert answer.data == DATA[131:]
    assert ranges(video)[0] == "bytes=131-230"


def test_no_range_is_a_plain_200_with_the_whole_song(video):
    answer = get()
    assert answer.status_code == 200
    assert "Content-Range" not in answer.headers
    assert answer.headers["Content-Length"] == "1024"
    assert answer.data == DATA


def test_a_wide_closed_range_ends_where_it_was_asked_to(video):
    answer = get("bytes=5-334")
    assert answer.status_code == 206
    assert answer.headers["Content-Range"] == "bytes 5-334/1024"
    assert answer.data == DATA[5:335]
    assert ranges(video) == ["bytes=5-104", "bytes=105-204", "bytes=205-304", "bytes=305-334"]


def test_a_range_within_one_piece_goes_upstream_as_it_is(video):
    answer = get("bytes=0-1")
    assert answer.status_code == 206
    assert answer.data == DATA[0:2]
    assert ranges(video) == ["bytes=0-1"]


def test_a_suffix_range_goes_upstream_as_it_is(video):
    answer = get("bytes=-5")
    assert answer.data == DATA[-5:]
    assert ranges(video) == ["bytes=-5"]


def test_a_url_that_runs_out_partway_is_resolved_again_once(video):
    video.refuse_next_from = 300
    answer = get("bytes=0-")
    assert answer.data == DATA
    assert video.resolved == ["vid-pieces", "vid-pieces"]
    assert video.asked[3] == ("https://googlevideo.example/1", "bytes=300-399")
    assert video.asked[4] == ("https://googlevideo.example/2", "bytes=300-399")


def test_a_piece_that_breaks_is_asked_for_again_from_where_it_stopped(video):
    video.break_next_from = 200
    answer = get("bytes=0-")
    assert answer.data == DATA
    assert "bytes=213-312" in ranges(video)


def test_a_range_past_the_end_is_still_a_416_with_the_size(video):
    answer = get("bytes=5000-")
    assert answer.status_code == 416
    assert answer.headers["Content-Range"] == "bytes */1024"


def test_a_piece_from_another_file_after_a_fresh_url_ends_the_connection(video):
    # The fresh URL names another format: its bytes must never follow the first file's.
    video.refuse_next_from = 300
    video.other_file_url = "/2"
    with pytest.raises(app._OtherFile):
        get("bytes=0-").data


def test_a_piece_that_cannot_be_opened_is_asked_for_again(video):
    video.unreachable_from = {200: 2}
    answer = get("bytes=0-")
    assert answer.data == DATA
    assert ranges(video).count("bytes=200-299") == 3


def test_a_piece_that_never_opens_ends_the_connection_instead_of_a_short_body(video):
    video.unreachable_from = {200: 10}
    with pytest.raises(ConnectionAbortedError):
        get("bytes=0-").data


def test_a_piece_that_answers_the_whole_file_is_not_used(video):
    video.whole_from = 500
    with pytest.raises(ConnectionAbortedError):
        get("bytes=0-").data
    assert all(piece.closed for piece in video.opened)


def test_with_no_content_range_on_the_first_piece_the_range_goes_up_as_it_came(video):
    video.no_range_from = {0}
    answer = get()
    assert answer.status_code == 200
    assert ranges(video)[:2] == ["bytes=0-99", "bytes=0-"]
    # The second answer is the plain path's: no Content-Range here either, so its length stands.
    assert answer.data == DATA


def test_a_head_closes_the_piece_it_opened(video):
    answer = app.app.test_client().head("/stream?id=vid-pieces", headers={"Range": "bytes=0-"})
    assert answer.status_code == 206
    assert answer.headers["Content-Length"] == "1024"
    assert answer.headers["Content-Range"] == "bytes 0-1023/1024"
    assert answer.data == b""
    assert all(piece.closed for piece in video.opened)
