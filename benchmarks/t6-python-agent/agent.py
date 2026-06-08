"""
Simule un agent LLM en état idle : dépendances chargées, boucle d'attente.
Reproduit le RSS d'un agent réel entre deux inférences.
"""
import langchain_core
import openai
import httpx
import pydantic
import time
import os
import signal

# Init minimale : crée les objets de base comme un vrai agent le ferait
client = openai.AsyncOpenAI(api_key="placeholder")

# Signale que l'agent est prêt (pour la synchronisation du benchmark)
print("READY", flush=True)

# Boucle idle — simule l'attente du prochain trigger
while True:
    time.sleep(10)
