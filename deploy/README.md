# Deployment (Raspberry Pi)

## Dependencies

```bash
sudo apt install cage libmpv-dev
```

## Cross-compile from x86 Linux

```bash
sudo apt install gcc-aarch64-linux-gnu
cargo build --target aarch64-unknown-linux-gnu --release
```

Or with Docker via `cross`:
```bash
cargo install cross
cross build --target aarch64-unknown-linux-gnu --release
```

Copy to Pi:
```bash
scp target/aarch64-unknown-linux-gnu/release/gallerypi pi@raspberrypi:~/gallerypi/
```

## Config

```bash
mkdir -p ~/.config/gallerypi
cp config.toml.example ~/.config/gallerypi/config.toml
# Edit media_dir to point to your pictures folder
```

## Kiosk mode

```bash
sudo cp deploy/gallerypi-kiosk.service /etc/systemd/system/
sudo systemctl enable gallerypi-kiosk
sudo systemctl start gallerypi-kiosk
```

## RPi `/boot/config.txt` recommendations

```ini
gpu_mem=128
```

## Manual launch (desktop)

```bash
RUST_LOG=gallerypi=info ./gallerypi
```

## Manual launch (kiosk, no desktop)

```bash
cage -- ./gallerypi
```
