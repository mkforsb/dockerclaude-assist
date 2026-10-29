SHELL=/bin/bash
export DCIMAGEPREFIX ?=
export DCPREFIX ?=

.PHONY: build clean setup-mounts start-container stop-container

build:
	docker build -t ${DCIMAGEPREFIX}dockerclaude:latest .

clean:
	docker image rm -f ${DCIMAGEPREFIX}dockerclaude:latest

# Binding ./mounts again would stack a new mount over, and hide, the existing
# project mounts, so only bind it once.
setup-mounts:
	mkdir -p ./${DCPREFIX}mounts ./${DCPREFIX}dot-claude
	mountpoint -q ./${DCPREFIX}mounts || sudo mount --bind ./${DCPREFIX}mounts ./${DCPREFIX}mounts
	sudo mount --make-rshared ./${DCPREFIX}mounts

start-container:
	DCIMAGETAG="$$(./dockerclaude.sh __image-tag)" docker compose -f ./docker-compose.yml up -d

stop-container:
	docker compose -f ./docker-compose.yml down
