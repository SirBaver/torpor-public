FROM python:3.11-slim

WORKDIR /app

# Deps typiques d'un agent LLM : orchestration + SDK + HTTP + validation
RUN pip install --no-cache-dir \
    langchain-core==0.3.* \
    openai==1.* \
    httpx==0.27.* \
    pydantic==2.* \
    && rm -rf /root/.cache

COPY agent.py .

CMD ["python", "agent.py"]
