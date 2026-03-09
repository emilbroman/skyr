.PHONY: image scoc-image deps compose up down

image:
	podman build -t skyr:latest -t localhost/skyr:latest .

scoc-image: image
	podman build -f Dockerfile.scoc -t skyr-scoc:latest -t localhost/skyr-scoc:latest .

deps:
	podman compose up -d scylla rabbitmq redis redpanda minio oci-registry buildkit
	@echo "Waiting for scylla to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_scylla_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

up: image scoc-image deps
	podman compose up -d --force-recreate api scs de rte-0 rte-1 rte-2 plugin-std-random plugin-std-artifact plugin-std-container scoc-1 scoc-2 scoc-3

down:
	podman compose down
