#!/usr/bin/env python3

import argparse
import json
import os
from pathlib import Path
from typing import Any

import whisper
from huggingface_hub import snapshot_download
import mlx_whisper


MODEL_CATALOG = {
    "mlx-tiny": {
        "name": "Whisper MLX Tiny",
        "family": "mlx",
        "size_mb": 151,
        "repo": "mlx-community/whisper-tiny",
    },
    "mlx-small": {
        "name": "Whisper MLX Small",
        "family": "mlx",
        "size_mb": 488,
        "repo": "mlx-community/whisper-small",
    },
    "mlx-medium": {
        "name": "Whisper MLX Medium",
        "family": "mlx",
        "size_mb": 1530,
        "repo": "mlx-community/whisper-medium",
    },
    "whisper-base": {
        "name": "Whisper Base",
        "family": "whisper",
        "size_mb": 142,
        "model_name": "base",
    },
    "whisper-small": {
        "name": "Whisper Small",
        "family": "whisper",
        "size_mb": 466,
        "model_name": "small",
    },
    "whisper-medium": {
        "name": "Whisper Medium",
        "family": "whisper",
        "size_mb": 1530,
        "model_name": "medium",
    },
}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    for command in ("list-models", "download-model"):
        command_parser = subparsers.add_parser(command)
        command_parser.add_argument("--models-dir", required=True)
        command_parser.add_argument("--active-model")
        if command == "download-model":
            command_parser.add_argument("--model-id", required=True)

    transcribe_parser = subparsers.add_parser("transcribe")
    transcribe_parser.add_argument("--models-dir", required=True)
    transcribe_parser.add_argument("--model-id", required=True)
    transcribe_parser.add_argument("--audio-path", required=True)
    transcribe_parser.add_argument("--output-path", required=True)
    transcribe_parser.add_argument("--language", default="en")
    transcribe_parser.add_argument("--diarize", action="store_true", default=False)

    clips_parser = subparsers.add_parser("extract-speaker-clips")
    clips_parser.add_argument("--audio-path", required=True)
    clips_parser.add_argument("--transcript-path", required=True)
    clips_parser.add_argument("--output-dir", required=True)

    return parser.parse_args()


def mlx_model_dir(models_dir: Path, model_id: str) -> Path:
    return models_dir / model_id


def whisper_model_root(models_dir: Path) -> Path:
    return models_dir / "whisper"


def whisper_model_file(models_dir: Path, model_id: str) -> Path:
    model_name = MODEL_CATALOG[model_id]["model_name"]
    return whisper_model_root(models_dir) / f"{model_name}.pt"


def is_installed(models_dir: Path, model_id: str) -> bool:
    config = MODEL_CATALOG[model_id]
    if config["family"] == "mlx":
        return (mlx_model_dir(models_dir, model_id) / "config.json").exists()
    return whisper_model_file(models_dir, model_id).exists()


def model_payload(models_dir: Path, model_id: str, active_model: str | None) -> dict[str, Any]:
    config = MODEL_CATALOG[model_id]
    installed = is_installed(models_dir, model_id)
    return {
        "id": model_id,
        "name": config["name"],
        "family": config["family"],
        "sizeMb": config["size_mb"],
        "installed": installed,
        "active": installed and model_id == active_model,
        "downloadProgress": None,
    }


def list_models(models_dir: Path, active_model: str | None) -> dict[str, Any]:
    models_dir.mkdir(parents=True, exist_ok=True)
    return {
        "models": [
            model_payload(models_dir, model_id, active_model)
            for model_id in MODEL_CATALOG
        ]
    }


def download_model(models_dir: Path, model_id: str, active_model: str | None) -> dict[str, Any]:
    if model_id not in MODEL_CATALOG:
        raise SystemExit(f"Unsupported model id: {model_id}")

    config = MODEL_CATALOG[model_id]
    models_dir.mkdir(parents=True, exist_ok=True)

    if config["family"] == "mlx":
        snapshot_download(
            repo_id=config["repo"],
            local_dir=mlx_model_dir(models_dir, model_id),
        )
    else:
        whisper_model_root(models_dir).mkdir(parents=True, exist_ok=True)
        whisper.load_model(
            config["model_name"],
            download_root=str(whisper_model_root(models_dir)),
            in_memory=False,
        )

    return {
        "model": model_payload(models_dir, model_id, active_model),
    }


def model_id_to_whisperx(model_id: str) -> str:
    """Map our model IDs to whisperx model names."""
    mapping = {
        "mlx-tiny": "tiny",
        "mlx-small": "small",
        "mlx-medium": "medium",
        "whisper-base": "base",
        "whisper-small": "small",
        "whisper-medium": "medium",
    }
    return mapping.get(model_id, "small")


def transcribe_with_diarization(
    audio_path: Path,
    model_id: str,
    models_dir: Path,
    language: str,
) -> dict[str, Any] | None:
    try:
        import whisperx  # type: ignore[import]
    except ImportError:
        return None

    import torch

    device = "mps" if torch.backends.mps.is_available() else "cpu"
    compute_type = "int8"
    whisperx_model_name = model_id_to_whisperx(model_id)
    normalized_language = None if language in ("", "auto") else language

    model = whisperx.load_model(whisperx_model_name, device, compute_type=compute_type)
    audio = whisperx.load_audio(str(audio_path))
    result = model.transcribe(audio, language=normalized_language)

    detected_language = result.get("language") or language or "en"

    try:
        model_a, metadata = whisperx.load_align_model(
            language_code=detected_language, device=device
        )
        result = whisperx.align(result["segments"], model_a, metadata, audio, device)
    except Exception:
        pass

    try:
        diarize_model = whisperx.DiarizationPipeline(device=device)
        diarize_segments = diarize_model(audio)
        result = whisperx.assign_word_speakers(diarize_segments, result)
    except Exception:
        pass

    return result


def normalize_segments(segments: list[dict[str, Any]], language: str) -> list[dict[str, Any]]:
    normalized = []
    for index, segment in enumerate(segments):
        # whisperx may attach speaker at the segment level or via words
        speaker: str | None = segment.get("speaker") or None
        if speaker is None:
            words = segment.get("words", [])
            for word in words:
                word_speaker = word.get("speaker")
                if word_speaker:
                    speaker = word_speaker
                    break
        normalized.append(
            {
                "id": str(segment.get("id", index)),
                "start": float(segment.get("start", 0.0)),
                "end": float(segment.get("end", 0.0)),
                "text": str(segment.get("text", "")).strip(),
                "speaker": speaker,
                "language": language,
            }
        )
    return normalized


def transcribe_audio(
    models_dir: Path,
    model_id: str,
    audio_path: Path,
    output_path: Path,
    language: str,
    diarize: bool = False,
) -> None:
    if model_id not in MODEL_CATALOG:
        raise SystemExit(f"Unsupported model id: {model_id}")
    if not audio_path.exists():
        raise SystemExit(f"Audio file not found: {audio_path}")
    if not is_installed(models_dir, model_id):
        raise SystemExit(f"Model {model_id} is not installed.")

    config = MODEL_CATALOG[model_id]
    normalized_language = None if language in ("", "auto") else language

    result: dict[str, Any] | None = None

    if diarize:
        result = transcribe_with_diarization(audio_path, model_id, models_dir, language)

    if result is None:
        if config["family"] == "mlx":
            result = mlx_whisper.transcribe(
                str(audio_path),
                path_or_hf_repo=str(mlx_model_dir(models_dir, model_id)),
                verbose=False,
                word_timestamps=False,
                language=normalized_language,
            )
        else:
            model = whisper.load_model(
                config["model_name"],
                download_root=str(whisper_model_root(models_dir)),
                in_memory=False,
            )
            result = model.transcribe(
                str(audio_path),
                language=normalized_language,
                verbose=False,
                fp16=False,
            )

    detected_language = result.get("language") or language or "en"
    payload = {
        "text": result.get("text", "").strip(),
        "language": detected_language,
        "segments": normalize_segments(result.get("segments", []), detected_language),
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(payload), encoding="utf-8")


def extract_speaker_clips(
    audio_path: Path,
    transcript_path: Path,
    output_dir: Path,
) -> dict[str, Any]:
    import subprocess

    if not audio_path.exists():
        raise SystemExit(f"Audio file not found: {audio_path}")
    if not transcript_path.exists():
        raise SystemExit(f"Transcript file not found: {transcript_path}")

    transcript = json.loads(transcript_path.read_text(encoding="utf-8"))
    segments = transcript.get("segments", [])

    # Collect all segments per speaker
    speaker_segments: dict[str, list[dict[str, Any]]] = {}
    for segment in segments:
        speaker = segment.get("speaker")
        if not speaker:
            continue
        speaker_segments.setdefault(speaker, []).append(segment)

    if not speaker_segments:
        return {"clips": []}

    output_dir.mkdir(parents=True, exist_ok=True)
    clips = []

    for speaker, segs in speaker_segments.items():
        # Find a representative clip: prefer segments >= 5 seconds, otherwise
        # pick the longest available one, but cap the clip at 10 seconds.
        best: dict[str, Any] | None = None
        for seg in segs:
            start = float(seg.get("start", 0.0))
            end = float(seg.get("end", 0.0))
            duration = end - start
            if duration >= 5.0:
                best = seg
                break
        if best is None:
            best = max(segs, key=lambda s: float(s.get("end", 0)) - float(s.get("start", 0)))

        clip_start = float(best.get("start", 0.0))
        clip_end = min(float(best.get("end", 0.0)), clip_start + 10.0)

        safe_label = speaker.lower().replace("_", "-").replace(" ", "-")
        clip_filename = f"{safe_label}.m4a"
        clip_path = output_dir / clip_filename

        subprocess.run(
            [
                "ffmpeg",
                "-y",
                "-i", str(audio_path),
                "-ss", str(clip_start),
                "-to", str(clip_end),
                "-vn",
                "-c:a", "aac",
                "-b:a", "128k",
                str(clip_path),
            ],
            check=True,
            capture_output=True,
        )

        clips.append(
            {
                "speaker": speaker,
                "file": clip_filename,
                "start": clip_start,
                "end": clip_end,
            }
        )

    return {"clips": clips}


def main() -> None:
    args = parse_args()

    if args.command == "list-models":
        models_dir = Path(args.models_dir).expanduser().resolve()
        print(json.dumps(list_models(models_dir, args.active_model)))
        return

    if args.command == "download-model":
        models_dir = Path(args.models_dir).expanduser().resolve()
        print(json.dumps(download_model(models_dir, args.model_id, args.active_model)))
        return

    if args.command == "extract-speaker-clips":
        result = extract_speaker_clips(
            audio_path=Path(args.audio_path).expanduser().resolve(),
            transcript_path=Path(args.transcript_path).expanduser().resolve(),
            output_dir=Path(args.output_dir).expanduser().resolve(),
        )
        print(json.dumps(result))
        return

    models_dir = Path(args.models_dir).expanduser().resolve()
    transcribe_audio(
        models_dir=models_dir,
        model_id=args.model_id,
        audio_path=Path(args.audio_path).expanduser().resolve(),
        output_path=Path(args.output_path).expanduser().resolve(),
        language=args.language,
        diarize=args.diarize,
    )
    print(json.dumps({"status": "ok"}))


if __name__ == "__main__":
    os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")
    main()
