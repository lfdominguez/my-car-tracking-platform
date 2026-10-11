# PaddleOCR sidecar for fuel-ticket reading. CPU only; needs AVX.
# paddlepaddle 3.3.x crashes in oneDNN on the detector (ConvertPirAttribute2RuntimeAttribute).
FROM python:3.12-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends libgomp1 libgl1 libglib2.0-0 curl \
    && rm -rf /var/lib/apt/lists/*

RUN pip install --no-cache-dir paddlepaddle==3.2.2 paddleocr==3.7.0

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
