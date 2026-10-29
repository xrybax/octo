"""Downloads preserve AAC audio in M4A, including after metadata is written."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

import pytest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
app_module = pytest.importorskip("app")


@pytest.fixture
def aac_source(tmp_path):
    if not shutil.which("ffmpeg") or not shutil.which("ffprobe"):
        pytest.skip("ffmpeg and ffprobe are required")
    source = tmp_path / "source.m4a"
    subprocess.run([
        "ffmpeg", "-hide_banner", "-loglevel", "error", "-y",
        "-f", "lavfi", "-i", "sine=frequency=440:duration=0.2",
        "-c:a", "aac", str(source),
    ], check=True)
    return source


def test_download_preserves_aac_and_tags_in_m4a(tmp_path, monkeypatch, aac_source):
    music = tmp_path / "music"
    dest = music / "Feel" / "A gdy jest już ciemno"
    monkeypatch.setattr(app_module, "_DOWNLOAD_ROOT", str(music))
    commands = []

    def download(args, **kwargs):
        commands.append((args, kwargs))
        output = args[args.index("-o") + 1].replace("%(ext)s", "m4a")
        shutil.copyfile(aac_source, output)
        return subprocess.CompletedProcess(args, 0, "", "")

    monkeypatch.setattr(app_module, "_run", download)
    with app_module.app.test_client() as client:
        response = client.get("/download", query_string={
            "id": "test-video", "dest": str(dest),
            "artist": "Feel", "title": "A gdy jest już ciemno",
        })
    assert response.status_code == 200
    output = str(dest) + ".m4a"
    assert response.json == {"path": output}
    args, options = commands[0]
    assert args[args.index("-f") + 1] == "141/140/bestaudio[ext=m4a]"
    assert "--audio-format" not in args and "-x" not in args
    assert options["bg"] is True
    assert not Path(str(dest) + ".mp3").exists()
    probe = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-show_streams", "-show_format", "-of", "json", output,
    ]))
    assert probe["streams"][0]["codec_name"] == "aac"
    assert probe["format"]["tags"]["artist"] == "Feel"
    assert probe["format"]["tags"]["title"] == "A gdy jest już ciemno"
    assert not Path(str(dest) + ".tagged.m4a").exists()


def test_download_failure_removes_partial_m4a(tmp_path, monkeypatch):
    monkeypatch.setattr(app_module, "_DOWNLOAD_ROOT", str(tmp_path))
    dest = tmp_path / "unfinished"

    def fail(args, **kwargs):
        Path(str(dest) + ".m4a").write_bytes(b"partial audio")
        return subprocess.CompletedProcess(args, 1, "", "unavailable")

    monkeypatch.setattr(app_module, "_run", fail)
    with app_module.app.test_client() as client:
        response = client.get("/download", query_string={"id": "test-video", "dest": str(dest)})
    assert response.status_code == 502
    assert response.json["expected"] == str(dest) + ".m4a"
    assert not Path(str(dest) + ".m4a").exists()
