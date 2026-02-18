<h1 align="center">GuGuGaGa</h1>

<p align="center">
  <strong>The supervision layer that makes Codex sessions safer, sharper, and consistently production-grade.</strong>
</p>

<p align="center">
  Built on top of <a href="https://github.com/openai/codex">OpenAI Codex</a> · Rust · TUI
</p>

---

GuGuGaGa is a real-time supervision layer for Codex that continuously evaluates agent behavior during live coding sessions and steers execution toward production-grade outcomes. Instead of replacing Codex, it wraps around it to add a second engineering judgment loop: catching risk, quality drift, and weak decisions early, enforcing stronger execution discipline without slowing momentum, preserving critical context across long runs, and keeping model and reasoning behavior predictable so results stay consistent under pressure.

The result is faster delivery with fewer avoidable regressions, cleaner implementation choices, and lower review overhead when projects get complex. GuGuGaGa is built for real codebases and production reliability, turning raw model speed into dependable engineering throughput by combining Codex's generation power with an always-on control system focused on correctness, stability, and sustained quality over time.
