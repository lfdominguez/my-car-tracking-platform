# PaddleOCR sidecar for fuel-ticket reading. CPU only (OpenVINO); needs AVX2.
FROM python:3.12-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends libgomp1 libgl1 libglib2.0-0 curl \
    && rm -rf /var/lib/apt/lists/*

# No paddlepaddle: the ONNX models run on ONNX Runtime's OpenVINO provider, so
# Paddle Inference (and its oneDNN crash in 3.3.x) is not involved. The plain
# onnxruntime wheel would shadow onnxruntime-openvino's module, so it goes.
RUN pip install --no-cache-dir paddleocr==3.7.0 onnxruntime-openvino==1.24.1 openvino==2026.4.1 \
    && (pip uninstall -y onnxruntime 2>/dev/null || true) \
    && python -c "import onnxruntime as o; assert 'OpenVINOExecutionProvider' in o.get_available_providers(), o.get_available_providers()"

RUN useradd --create-home ocr
USER ocr
WORKDIR /home/ocr
COPY server.py .

ARG OCR_LANG=es
ENV OCR_LANG=${OCR_LANG} PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK=True
# Download the models at build time so the container starts offline.
RUN python -c "import server; server.build()"

EXPOSE 8866
HEALTHCHECK --interval=30s --timeout=5s --start-period=60s CMD curl -fs http://127.0.0.1:8866/health || exit 1
CMD ["python", "server.py"]
