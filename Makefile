SHELL=/bin/bash
export DCIMAGEPREFIX ?=
export DCPREFIX ?=

.PHONY: build clean setup-mounts start-container stop-container

build:
	docker build -t ${DCIMAGEPREFIX}dockerclaude:latest .

clean:
	docker image rm -f ${DCIMAGEPREFIX}dockerclaude:latest

setup-mounts:
	mkdir -p ./${DCPREFIX}mounts ./${DCPREFIX}dot-claude
	sudo mount --bind ./${DCPREFIX}mounts ./${DCPREFIX}mounts
	sudo mount --make-rshared ./${DCPREFIX}mounts

start-container:
	DCIMAGETAG="$$(./dockerclaude.sh __image-tag)" docker compose -f ./docker-compose.yml up -d

stop-container:
	docker compose -f ./docker-compose.yml down
