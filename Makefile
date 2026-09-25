.PHONY: build clean setup-mounts start-container stop-container

build:
	docker build -t dockerclaude:latest .

clean:
	docker image rm -f dockerclaude:latest

setup-mounts:
	sudo mount --bind ./mounts ./mounts
	sudo mount --make-rshared ./mounts

start-container:
	docker compose -f ./docker-compose.yml up -d

stop-container:
	docker compose -f ./docker-compose.yml down
