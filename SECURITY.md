# Security Policy

## Supported Versions

| Version | Supported          |
|---------|--------------------|
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

If you discover a security vulnerability in `quilt-jetson`, please
report it privately:

- **Email**: security@superinstance.dev
- **GitHub Security Advisories**: [create a private advisory](https://github.com/SuperInstance/quilt-jetson/security/advisories/new)

Please do **not** file a public issue for security vulnerabilities.

We will acknowledge receipt within 48 hours and aim to provide a
fix or mitigation within 14 days for critical issues, 30 days for
high-severity issues, and 90 days for lower-severity issues.

## What to Include

When reporting a vulnerability, please include:

1. A description of the vulnerability and its impact.
2. Steps to reproduce the issue.
3. The version(s) of `quilt-jetson` affected.
4. Any known mitigations or workarounds.

## Scope

The following are in scope:

- The Rust crate and its dependencies.
- The CLI binary `quilt-jetson`.
- The web UI (HTML/JS).
- The example YAML sheets.
- The CI/CD workflows.

The following are **out of scope**:

- The `rclrs` dependency (file issues upstream).
- The `ort` (ONNX Runtime) dependency (file issues upstream).
- Vulnerabilities in the user's ROS2 daemon or ONNX model files.

## Security Best Practices

When deploying `quilt-jetson`:

1. **Don't expose the web UI to the public internet** without
   putting it behind a reverse proxy with TLS and auth. The
   default `:8080` server is unauthenticated.
2. **Use bearer tokens for federation**. The
   `QuiltRef::with_token()` API sets a token that's sent on
   every request. Store tokens in environment variables, not
   in sheets.
3. **Pin your ONNX/TensorRT models**. A malicious model file
   can execute arbitrary code when loaded by `ort`. Only load
   models from trusted sources.
4. **Lock down GPIO/I2C/SPI permissions**. Sensor cells with
   `source: imu:i2c-1:0x68` etc. require hardware access.
   Run `quilt-jetson` as a user with the necessary group
   membership, not as root.
5. **Review your YAML sheets before loading**. The CLI
   accepts any YAML, including `program` cells with rhai
   scripts. An attacker who can write to a sheet can run
   arbitrary code. Treat sheet files like code.

## Cryptography

`quilt-jetson` does not implement any cryptography directly.
The `web` module's HTTP and WebSocket servers run over plain
HTTP. Use a reverse proxy (nginx, caddy) for TLS termination.

## Acknowledgements

We follow responsible disclosure and will credit reporters in
our release notes (unless they prefer to remain anonymous).
