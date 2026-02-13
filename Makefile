.PHONY: image compose-full compose-deps wait-deps

image:
	podman build -t skyr:latest -t localhost/skyr:latest .

deps:
	podman compose up -d cassandra rabbitmq
	@echo "Waiting for cassandra to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_cassandra_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

compose: image deps
	podman compose up de scs rte-0 rte-1 rte-2
