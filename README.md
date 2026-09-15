# Ladybird Reports

Ladybird Reports accepts anonymous diagnostic reports and provides a private
interface for triage and issue management. It is designed to support different
report types, client versions, typed fields, and PNG attachments.

The application runs as two services from one container image:

- `ladybird-reports-public-api` accepts untrusted submissions with a restricted
  PostgreSQL role.
- `ladybird-reports-admin` migrates the database and serves the management UI
  with the administrative PostgreSQL role.

Run the complete local test suite with:

```sh
./scripts/test-local.sh
```

## Documentation

- [Architecture](docs/architecture.md)
- [Public API protocol](docs/protocol-v1.md)
- [Management interface](docs/management.md)
- [Deployment](docs/deployment.md)
- [Development and testing](docs/development.md)
- [Operations](docs/operations.md)
- [Threat model](docs/threat-model.md)

## License

Ladybird Reports is licensed under the [BSD 2-Clause License](LICENSE).
