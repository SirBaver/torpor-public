FROM python:3.12-alpine

WORKDIR /app

RUN pip install --no-cache-dir fastapi "uvicorn[standard]" requests

COPY daemon/ ./daemon/
COPY config/ ./config/

RUN mkdir -p /app/data /app/logs

CMD ["sh", "-c", "python -m uvicorn daemon.main:app --host 0.0.0.0 --port ${DAEMON_PORT:-8888}"]
