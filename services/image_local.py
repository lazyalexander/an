"""Loopback image service for An's local image model nodes.

Keeps SDXL base 1.0 with the xinsir ControlNet scribble model resident on
Apple Silicon (MPS) and answers An's host-side image calls:

  GET  /health    -> {"model": NAME}
  POST /generate  {"model", "prompt", "size", "image": base64 PNG or null}
                  -> {"image": base64 PNG, "usage": {...}}

With an image, it is used as the scribble control (dark lines on white).
Without one, a blank control lets the prompt alone decide.

Run in a Python environment with torch (MPS), diffusers, transformers,
accelerate and the weights in the Hugging Face cache:
  python services/image_local.py [--port 11435]
It binds 127.0.0.1 only; An is its only intended client.
"""
import argparse
import base64
import io
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import torch
from diffusers import AutoencoderKL, ControlNetModel, StableDiffusionXLControlNetPipeline
from PIL import Image, ImageOps

NAME = "sdxl-base-1.0+controlnet-scribble"
NEGATIVE = "blurry, low quality, text, watermark, extra objects, cluttered background"


def load():
    controlnet = ControlNetModel.from_pretrained("xinsir/controlnet-scribble-sdxl-1.0",
                                                 torch_dtype=torch.float16)
    vae = AutoencoderKL.from_pretrained("madebyollin/sdxl-vae-fp16-fix", torch_dtype=torch.float16)
    pipe = StableDiffusionXLControlNetPipeline.from_pretrained(
        "stabilityai/stable-diffusion-xl-base-1.0", controlnet=controlnet, vae=vae,
        torch_dtype=torch.float16, variant="fp16").to("mps")
    pipe.set_progress_bar_config(disable=True)
    return pipe


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--port", type=int, default=11435)
    parser.add_argument("--steps", type=int, default=25)
    args = parser.parse_args()
    t0 = time.monotonic()
    pipe = load()
    print(f"image service: {NAME} loaded in {time.monotonic() - t0:.0f}s", flush=True)
    gpu = threading.Lock()  # one generation at a time on the GPU

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, status, body):
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            if self.path == "/health":
                return self.reply(200, {"model": NAME})
            self.reply(404, {"error": "not found"})

        def do_POST(self):
            if self.path != "/generate":
                return self.reply(404, {"error": "not found"})
            try:
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                if request.get("model") != NAME:
                    return self.reply(400, {"error": f"this service runs {NAME}"})
                width, height = (int(v) for v in request.get("size", "1024x1024").split("x"))
                if request.get("image"):
                    control = Image.open(io.BytesIO(base64.b64decode(request["image"]))).convert("RGB")
                    control = ImageOps.pad(control, (width, height), color="white")
                else:
                    control = Image.new("RGB", (width, height), "white")
            except Exception as exc:  # noqa: BLE001 — malformed request
                return self.reply(400, {"error": str(exc)})
            with gpu:
                t1 = time.monotonic()
                image = pipe(prompt=request["prompt"], negative_prompt=NEGATIVE, image=control,
                             controlnet_conditioning_scale=0.9 if request.get("image") else 0.0,
                             num_inference_steps=args.steps, guidance_scale=6.0,
                             width=width, height=height).images[0]
                seconds = round(time.monotonic() - t1, 1)
            out = io.BytesIO()
            image.save(out, "PNG")
            self.reply(200, {"image": base64.b64encode(out.getvalue()).decode(),
                             "usage": {"seconds": seconds, "steps": args.steps}})

    with ThreadingHTTPServer(("127.0.0.1", args.port), Handler) as server:
        print(f"image service: listening on 127.0.0.1:{args.port}", flush=True)
        server.serve_forever()


if __name__ == "__main__":
    main()
