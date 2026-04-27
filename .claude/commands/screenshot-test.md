# Screenshot Test

Run the headless screenshot test suite and open the HTML report.

```bash
cargo run --bin screenshot_test
```

The binary renders all test scenarios headlessly (no display needed) using Slint's software renderer with gradient mock images, then writes:
- `screenshots/*.png` — one PNG per scenario
- `screenshots/report.html` — self-contained HTML report with all screenshots embedded

To add a new scenario, add a builder function and register it in the `scenarios` slice in `src/bin/screenshot_test.rs`.
