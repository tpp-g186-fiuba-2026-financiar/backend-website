up:
	docker compose build --no-cache && docker compose up

test:
	docker compose -f docker-compose.test.yml up --abort-on-container-exit
	docker compose -f docker-compose.test.yml down

coverage:
	cargo llvm-cov --all-targets --summary-only --ignore-filename-regex '(main\.rs|configuration/config\.rs|database/db\.rs)' --fail-under-lines 80

down:
	docker compose down
	docker compose -f docker-compose.test.yml down
