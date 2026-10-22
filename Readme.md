# FinanciAr Backend

## Requisitos
- Docker
- Docker Compose
- Un archivo `.env` configurado correctamente

## Variables necesarias
El proyecto usa variables de entorno para la base de datos, JWT, SMTP y CORS. Algunas de las más relevantes son:

- `DATABASE_URL`
- `PORT` (por defecto `8500`)
- `JWT_SECRET`
- `JWT_EXPIRATION_HOURS`
- `ALLOWED_ORIGIN`
- `DATA_COLLECTOR_URL`
- `API_ML_URL`
- `MODAL_URL`: template de las URLs de los modelos en Modal, con `{endpoint}` donde va
  `<app>-<funcion>` (p. ej. `https://financiar186--{endpoint}.us-east.modal.run`). Cambiar de
  workspace de Modal es cambiar solo esta variable. Cada modelo se puede pisar por separado con
  su propia variable (`MODAL_LSTM_URL`, `MODAL_SVM_URL`, ...).
- `SMTP_HOST`
- `SMTP_PORT`
- `SMTP_USERNAME`
- `SMTP_PASSWORD`
- `SMTP_FROM_EMAIL`
- `SMTP_FROM_NAME`

Importante: cuando se corre localmente, `ALLOWED_ORIGIN` debe apuntar al frontend que está consumiendo la API (por ejemplo `http://localhost:5173`).

## Cómo levantar el proyecto
1. Configurar el archivo `.env` con los valores adecuados.
2. Ejecutar:

```bash
make up
```

Esto construye la imagen y levanta los contenedores de PostgreSQL y la API.

La API queda disponible en:
- `http://localhost:8500`
- Swagger: `http://localhost:8500/swagger`
- Health check: `http://localhost:8500/health`

La base de datos queda expuesta en el puerto:
- PostgreSQL: `localhost:5433`

## Cómo detener el entorno

```bash
make down
```

## Cómo correr tests

```bash
make test
```

## Cómo correr cobertura

```bash
make coverage
```