SHELL=/bin/bash

.PHONY: build clean setup-mounts start-container stop-container dot-claude-clean

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

dot-claude-clean:
	@echo "Really wipe $$(realpath .claude)?"
	@read -p "Y/n? " -n 1 ans </dev/tty; \
	if [ "$$ans" = "Y" ]; then \
		echo; \
		IFS=$$'\n'; \
		for f in $$( \
			find ".claude" -maxdepth 1 -mindepth 1 \
			! -name ".credentials.json" \
			! -name ".claude.json" \
			! -name "settings.json" \
			! -name "skills" \
			! -name ".gitkeep" \
		); do rm -vrf "$$f"; \
		done; \
	else \
		echo; \
	fi
