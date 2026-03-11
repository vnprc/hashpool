# Testnet4 Deploy Plan (Pre-Alpha, Debian VPS)

Date: 2026-03-11

Goal: keep the deployment simple and reliable for a demo. Avoid Nix on the VPS. Build binaries against Debian 12 ABI and ship them.

## Recommended Approach (Minimal Engineering)

1) **Build host matches VPS runtime**
   - Use a Debian 12 x86_64 build host (local VM or a temporary larger VPS).
   - Build once, then ship binaries to the demo VPS.

2) **Ship-only deploy path**
   - Add a deploy flag to skip builds (e.g., `--skip-build` or `BUILD_MODE=ship`).
   - The deploy script should only stage binaries/configs and install services.

3) **Safety guard against Nix ABI**
   - Add a check that refuses binaries referencing `/nix/store/.../ld-linux`.
   - This avoids broken deploys on Debian.

4) **Deploy to VPS**
   - Run `./scripts/deploy.sh --skip-build` from the dev VM.
   - This rsyncs binaries + configs + systemd + nginx.

5) **Validate**
   - `systemctl status hashpool-*`
   - `nginx -t && systemctl reload nginx`
   - `journalctl -u hashpool-bitcoin-node -f`
   - `journalctl -u hashpool-sv2-tp -f`

## Optional Fallback (If Builds Are Too Slow)

- Temporarily scale VPS to 4-8 vCPU / 8-16 GB RAM, build once, then downgrade.
- Or build on a larger temporary Debian 12 box and ship artifacts.

## Notes

- Debian 12 is fine; the issue was ABI mismatch (Nix-built binaries on Debian).
- A single SAN cert in `/etc/letsencrypt/live/pool.hashpool.dev/` is valid for pool/proxy/mint/wallet.
- Nginx configs should point at that cert path consistently.
