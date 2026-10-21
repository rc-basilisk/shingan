#!/usr/bin/env python3
"""Regenerate crates/shingan-ml/data/clip_prototypes.bin.

Each image category is described by several short prompts; their CLIP text
embeddings are L2-normalized, averaged and re-normalized ("prompt ensembling",
as in the CLIP paper). The result is 24 x 512 little-endian float32 values in
ImageSubCategory::ALL order.

Requirements: pip install onnxruntime tokenizers numpy
Model files (from https://huggingface.co/Xenova/clip-vit-base-patch32):
  onnx/text_model.onnx and tokenizer.json, placed in MODEL_DIR.

Usage: python tools/gen_clip_prototypes.py MODEL_DIR [OUTPUT]
"""

import sys

import numpy as np
import onnxruntime as ort
from tokenizers import Tokenizer

# Order must match ImageSubCategory::ALL in crates/shingan-ml/src/taxonomy.rs.
PROMPTS = {
    "screenshot_desktop": [
        "a screenshot of a computer desktop",
        "a screenshot of a web browser window",
        "a screen capture of a desktop application",
        "a screenshot of a code editor on a monitor",
        "a screenshot of a windows or mac desktop with windows and icons",
    ],
    "screenshot_mobile": [
        "a screenshot of a phone screen",
        "a screenshot of a mobile app",
        "a smartphone screenshot of a chat conversation",
        "a screenshot of a social media app on a phone",
        "a tall screenshot of an iphone or android interface",
    ],
    "screen_recording_frame": [
        "a frame from a screen recording",
        "a blurry still from a video of a computer screen",
        "a frame from a software tutorial video",
        "a paused video game frame",
    ],
    "photo_landscape": [
        "a photo of a landscape",
        "a photo of mountains",
        "a photo of a beach and the ocean",
        "a photo of a forest",
        "a scenic travel photo of nature",
        "a photo of a sunset over a lake",
    ],
    "photo_portrait": [
        "a photo of a person",
        "a portrait photo of a face",
        "a selfie",
        "a headshot of a man",
        "a headshot of a woman",
        "a photo of a smiling person",
    ],
    "photo_food": [
        "a photo of food",
        "a photo of a meal on a plate",
        "a photo of a dish at a restaurant",
        "a photo of a drink",
        "a photo of dessert",
    ],
    "photo_animal": [
        "a photo of an animal",
        "a photo of a pet",
        "a photo of a dog",
        "a photo of a cat",
        "a wildlife photo",
        "a photo of a bird",
    ],
    "photo_architecture": [
        "a photo of a building",
        "a photo of a famous landmark",
        "a photo of a city street",
        "a photo of architecture",
        "a photo of a skyline",
        "a photo of a room interior",
        "a photo of a tower",
    ],
    "photo_event": [
        "a photo of a group of people at a party",
        "a photo of a concert crowd",
        "a photo of a wedding",
        "a photo of people at an event",
        "a group photo",
    ],
    "photo_product": [
        "a product photo on a white background",
        "a photo of an object on a table",
        "a close-up photo of a product",
        "a macro photo of a small object",
        "a photo of an item for sale",
    ],
    "photo_general": [
        "a photo",
        "a casual snapshot",
        "an ordinary photo taken with a phone camera",
        "a photo of everyday things",
    ],
    "artwork": [
        "a painting",
        "a digital painting",
        "an illustration",
        "concept art",
        "an oil painting",
        "a watercolor painting",
        "a famous painting in a museum",
    ],
    "anime_manga": [
        "an anime illustration",
        "a manga drawing",
        "anime fan art",
        "an anime character",
        "a screenshot from an anime",
    ],
    "pixel_art": [
        "pixel art",
        "a retro video game sprite",
        "8-bit pixel graphics",
        "a pixelated game scene",
    ],
    "comic": [
        "a comic book page",
        "a comic strip with panels and speech bubbles",
        "a webcomic",
        "a cartoon strip",
    ],
    "render_3d": [
        "a 3d render",
        "a cgi image",
        "a computer generated 3d scene",
        "a blender render",
        "a 3d model rendering",
    ],
    "meme": [
        "a meme",
        "an internet meme with text",
        "a funny image with a caption",
        "a reaction image",
        "a meme with impact font text",
    ],
    "diagram": [
        "a diagram",
        "a flowchart",
        "a technical schematic",
        "an architecture diagram with boxes and arrows",
        "a mind map",
    ],
    "infographic_chart": [
        "a bar chart",
        "a line graph",
        "a pie chart",
        "an infographic",
        "a data visualization",
    ],
    "scanned_document": [
        "a scanned document",
        "a page of printed text",
        "a scanned receipt",
        "an invoice",
        "a photo of a printed letter",
    ],
    "handwritten": [
        "handwritten notes",
        "handwriting on paper",
        "a photo of a whiteboard",
        "a pencil sketch on paper",
        "a page from a notebook",
    ],
    "ui_design": [
        "a user interface mockup",
        "a website wireframe",
        "an app design in figma",
        "a ui design layout",
    ],
    "logo_icon": [
        "a logo",
        "an app icon",
        "a simple icon",
        "a company logo on a plain background",
        "an emoji",
    ],
    "other": [
        "an abstract image",
        "a texture",
        "a blank image",
        "a random image",
    ],
}


def main() -> None:
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    model_dir = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else "crates/shingan-ml/data/clip_prototypes.bin"

    tok = Tokenizer.from_file(f"{model_dir}/tokenizer.json")
    sess = ort.InferenceSession(f"{model_dir}/text_model.onnx")

    def embed(text: str) -> np.ndarray:
        ids = np.array([tok.encode(text).ids], dtype=np.int64)
        (e,) = sess.run(["text_embeds"], {"input_ids": ids})
        e = e[0]
        return e / np.linalg.norm(e)

    rows = []
    for label, prompts in PROMPTS.items():
        mean = np.mean([embed(p) for p in prompts], axis=0)
        rows.append(mean / np.linalg.norm(mean))
        print(f"{label:<24} {len(prompts)} prompts")

    arr = np.stack(rows).astype("<f4")
    assert arr.shape == (24, 512), arr.shape
    arr.tofile(out)
    print(f"wrote {out}")


if __name__ == "__main__":
    main()
