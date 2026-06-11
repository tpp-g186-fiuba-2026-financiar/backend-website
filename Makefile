up:
	docker compose build --no-cache && docker compose up

test:
	docker compose -f docker-compose.test.yml up --abort-on-container-exit
	docker compose -f docker-compose.test.yml down

down:
	docker compose down
	docker compose -f docker-compose.test.yml down