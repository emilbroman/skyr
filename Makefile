.PHONY: image scoc-image web-image deps compose up down build-cli install-cli uninstall-cli cloud-config

image:
	podman build -f dev/Containerfile.skyr -t skyr:latest -t localhost/skyr:latest .

scoc-image: image
	podman build -f dev/Containerfile.scoc -t skyr-scoc:latest -t localhost/skyr-scoc:latest dev

web-image:
	podman build -f dev/Containerfile.web -t skyr-web:latest -t localhost/skyr-web:latest .

deps:
	podman compose -f dev/podman-compose.yml up -d scylla rabbitmq redis redpanda minio oci-registry buildkit
	@echo "Waiting for scylla to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_scylla_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

up: image scoc-image web-image deps
	podman compose -f dev/podman-compose.yml up -d --force-recreate web api scs de rte-0 rte-1 rte-2 plugin-std-random plugin-std-time plugin-std-artifact plugin-std-crypto plugin-std-container scoc-1 scoc-2 scoc-3

down:
	podman compose -f dev/podman-compose.yml down

build-cli:
	cargo build --release -p cli

install-cli: build-cli
	sudo install target/release/skyr /usr/local/bin

uninstall-cli:
	sudo rm /usr/local/bin/skyr

cloud-config:
	envsubst < infra/scoc-cloud-config.yaml
