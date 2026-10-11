"""PaddleOCR sidecar for fuel-ticket reading.

POST /ocr with a raw image body -> {"lines": [{"text", "score", "box": [x1, y1, x2, y2]}]}
GET /health -> {"status": "ok"}

Single-threaded on purpose: one Paddle pipeline, one photo at a time. Other
requests wait in the listen backlog. Only reachable on the internal network.
"""

import json
import os
from http.server import BaseHTTPRequestHandler, HTTPServer

import cv2
import numpy as np
from paddleocr import PaddleOCR

MAX_BYTES = 8 * 1024 * 1024
MIN_SCORE = float(os.environ.get("OCR_MIN_SCORE", "0.5"))


def build():
    # Accuracy over speed: Paddle picks its best models for the language, plus
    # page orientation, unwarping of curled paper and per-line orientation.
    # Inference runs the ONNX exports on OpenVINO's CPU backend: same text as
    # Paddle Inference, ~35 % faster per photo and 5 s instead of ~30 s to load
    # on an i5-6400T. (The iGPU was slower, and hung in FP16.)
    threads = int(os.environ.get("OCR_CPU_THREADS", os.cpu_count() or 4))
    return PaddleOCR(
        lang=os.environ.get("OCR_LANG", "es"),
        use_doc_orientation_classify=True,
        use_doc_unwarping=True,
        use_textline_orientation=True,
        engine="onnxruntime",
        engine_config={
            "providers": ["OpenVINOExecutionProvider"],
            "provider_options": [{"device_type": "CPU", "precision": "FP32", "num_of_threads": threads}],
        },
    )


def read(ocr, data):
    img = cv2.imdecode(np.frombuffer(data, np.uint8), cv2.IMREAD_COLOR)
    if img is None:
        raise ValueError("not an image")
    lines = []
    for res in ocr.predict(img):
        for text, score, box in zip(res["rec_texts"], res["rec_scores"], res["rec_boxes"]):
            if text.strip() and score >= MIN_SCORE:
                lines.append({"text": text, "score": round(float(score), 3), "box": [float(v) for v in box]})
    return lines


class Handler(BaseHTTPRequestHandler):
    def _send(self, code, body):
        raw = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/health":
            self._send(200, {"status": "ok"})
        else:
            self._send(404, {"error": "not found"})

    def do_POST(self):
        if self.path != "/ocr":
            return self._send(404, {"error": "not found"})
        length = int(self.headers.get("Content-Length") or 0)
        if not 0 < length <= MAX_BYTES:
            return self._send(413 if length else 400, {"error": "bad body size"})
        data = self.rfile.read(length)
        try:
            self._send(200, {"lines": read(OCR, data)})
        except ValueError as e:
            self._send(400, {"error": str(e)})
        except Exception as e:  # keep serving; the backend maps this to 503
            print(f"ocr: failed: {e!r}", flush=True)
            self._send(500, {"error": "ocr failed"})

    def log_message(self, fmt, *args):
        print("ocr: " + fmt % args, flush=True)


if __name__ == "__main__":
    OCR = build()
    port = int(os.environ.get("PORT", "8866"))
    print(f"ocr: listening on :{port}", flush=True)
    HTTPServer(("0.0.0.0", port), Handler).serve_forever()
