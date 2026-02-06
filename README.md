<p align="center">
  <img src="assets/logo.png" width="200" />
</p>

<h1 align="center">GuGuGaGa</h1>

<p align="center">
  <strong>A supervisor agent that monitors, evaluates, and corrects AI coding agents in real-time.</strong>
</p>

<p align="center">
  Built on top of <a href="https://github.com/openai/codex">OpenAI Codex</a> · Rust · TUI
</p>

---

GuGuGaGa wraps around Codex's app-server protocol as a transparent proxy, intercepting every message between you and the agent. It watches for sloppy behavior — placeholder code, skipped error handling, ignored instructions — and either corrects the agent automatically or escalates to you.

Think of it as a senior engineer sitting next to your AI, keeping it honest.
