# Pollux Migrations

## Usage 🌟

Install sqlx cli tool:

```sh
cargo install sqlx-cli
```

Source `.env` or set `$DATABASE_URL` manually:

```sh
. .env
```

Generate db if not already done:

```sh
$CARGO_HOME/bin/sqlx db create
```

Then use the executable:

```sh
$CARGO_HOME/bin/sqlx migrate run
```
