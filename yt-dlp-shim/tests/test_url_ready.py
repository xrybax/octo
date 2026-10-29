"""A resolved URL is fetched no sooner than its `available_at`, and stream URLs come from the
player client that hands out ready ones, with yt-dlp's own client behind it."""
import os
import sys
import time

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import facts  # noqa: E402

app = pytest.importorskip("app")

URL = "https://rr1---sn-x.googlevideo.com/videoplayback?itag=140&mime=audio%2Fmp4&clen=4072024"


def test_the_format_line_says_when_its_url_may_first_be_fetched():
    line = "140|4072024|m4a|129.538|1792637357"
    assert facts.is_format_line(line)
    assert facts.parse_format_line(line) == {"itag": "140", "size": 4072024, "ext": "m4a", "abr": 130}
    assert facts.available_at(line) == 1792637357.0
    assert facts.available_at("140|4072024|m4a|129.538|NA") is None
    assert facts.available_at("140|4072024|m4a|129.538") is None
    rest, format_line, url = facts.split_run('{"id": "x"}\n' + line + "\n" + URL + "\n")
    assert format_line == line and url == URL and rest == ['{"id": "x"}']


@pytest.fixture
def no_resting(monkeypatch):
    monkeypatch.setattr(app, "_PLAYER_CLIENT", "visionos")
    monkeypatch.setattr(app, "_PLAYER_CLIENT_RESTING_UNTIL", 0.0)


def test_runs_ask_the_player_client(monkeypatch, no_resting):
    calls = []
    monkeypatch.setattr(app, "_run", lambda args, **k: calls.append(args) or "140|1|m4a|1|NA\n" + URL + "\n")
    app._run_with_client(["-f", "140", "x"], label="resolve", bg=False)
    assert len(calls) == 1
    assert calls[0][:2] == ["--extractor-args", "youtube:player_client=visionos"]


def test_no_url_from_the_client_asks_yt_dlps_own_and_rests_the_client(monkeypatch, no_resting):
    calls = []

    def run(args, **k):
        calls.append(args)
        return None if "--extractor-args" in args else "140|1|m4a|1|NA\n" + URL + "\n"

    monkeypatch.setattr(app, "_run", run)
    out = app._run_with_client(["-f", "140", "x"], label="resolve", bg=False)
    assert facts.split_run(out)[2] == URL
    assert calls == [["--extractor-args", "youtube:player_client=visionos", "-f", "140", "x"], ["-f", "140", "x"]]
    # Resting: the next run goes straight to yt-dlp's own client.
    calls.clear()
    app._run_with_client(["-f", "140", "y"], label="resolve", bg=False)
    assert calls == [["-f", "140", "y"]]


def test_a_video_both_clients_fail_on_leaves_the_client_in_use(monkeypatch, no_resting):
    monkeypatch.setattr(app, "_run", lambda args, **k: None)
    assert app._run_with_client(["-f", "140", "x"], label="resolve", bg=False) is None
    assert app._client_args() == ["--extractor-args", "youtube:player_client=visionos"]


def test_an_empty_client_uses_yt_dlps_own(monkeypatch):
    monkeypatch.setattr(app, "_PLAYER_CLIENT", "")
    calls = []
    monkeypatch.setattr(app, "_run", lambda args, **k: calls.append(args) or None)
    app._run_with_client(["-f", "140", "x"], label="resolve", bg=False)
    assert calls == [["-f", "140", "x"]]


def test_a_resolve_keeps_the_time_its_url_may_first_be_fetched(monkeypatch, no_resting):
    at = time.time() + 5
    monkeypatch.setattr(app, "_run", lambda args, **k: f"140|4072024|m4a|129.5|{at}\n{URL}&v=ready1\n")
    app._url_cache_evict("vid-ready01")
    assert app._resolve_url("vid-ready01") == URL + "&v=ready1"
    slept = []
    monkeypatch.setattr(app.time, "sleep", slept.append)
    app._wait_until_ready(URL + "&v=ready1", "vid-ready01")
    assert len(slept) == 1 and 4 < slept[0] <= 5


def test_the_wait_is_bounded_and_skipped_once_ready(monkeypatch):
    slept = []
    monkeypatch.setattr(app.time, "sleep", slept.append)
    monkeypatch.setattr(app, "_URL_READY_WAIT_MAX_SEC", 2.0)
    app._url_ready_put(URL + "&v=far", time.time() + 60)
    app._url_ready_put(URL + "&v=past", time.time() - 1)
    app._wait_until_ready(URL + "&v=far", "vid")
    app._wait_until_ready(URL + "&v=past", "vid")
    app._wait_until_ready(URL + "&v=unknown", "vid")
    assert slept == [2.0]


def test_every_upstream_open_waits_for_its_url(monkeypatch):
    order = []
    monkeypatch.setattr(app, "_wait_until_ready", lambda url, video_id: order.append("wait"))

    class Session:
        def get(self, url, **kwargs):
            order.append("get")
            return "answer"

    monkeypatch.setattr(app, "_SESSION", Session())
    assert app._open_upstream(URL, {}, "vid") == "answer"
    assert order == ["wait", "get"]
