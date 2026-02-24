.PHONY: image deps compose dev

image:
	podman build -t skyr:latest -t localhost/skyr:latest .

deps: image
	podman compose up -d scylla rabbitmq redis redpanda plugin-std-random
	@echo "Waiting for scylla to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_scylla_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

compose: image deps
	podman compose up de scs rte-0 rte-1 rte-2 api

dev:
	nix develop -c cargo watch -s 'set -e; \
		cargo run -p plugin_std_random -- --bind tcp://127.0.0.1:50051 & plugin_pid=$$!; \
		cargo run -p api -- --host 127.0.0.1 --port 8080 --challenge-salt local-dev-challenge-salt & api_pid=$$!; \
		cargo run -p scs -- daemon --address 127.0.0.1:2222 --key host.pem & scs_pid=$$!; \
		cargo run -p de -- daemon & de_pid=$$!; \
		cargo run -p rte -- daemon --plugin Std/Random@tcp://127.0.0.1:50051 --worker-index 0 --worker-count 3 --local-workers 3 & rte_pid=$$!; \
		trap "kill $$plugin_pid $$api_pid $$scs_pid $$de_pid $$rte_pid 2>/dev/null || true" EXIT INT TERM; \
		wait'
