.PHONY: image scoc-image web-image deps compose up down deps-multi-region up-multi-region down-multi-region build-cli install-cli uninstall-cli cloud-config spec spec-watch spec-clean

image:
	podman build -f dev/Containerfile.skyr -t skyr:latest -t localhost/skyr:latest .

scoc-image: image
	podman build -f dev/Containerfile.scoc -t skyr-scoc:latest -t localhost/skyr-scoc:latest dev

web-image:
	podman build -f dev/Containerfile.web -t skyr-web:latest -t localhost/skyr-web:latest .

deps:
	podman compose -f dev/podman-compose.yml up -d scylla rabbitmq redis redpanda minio oci-registry buildkit mailhog
	@echo "Waiting for scylla to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_scylla_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr_rabbitmq_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

up: image scoc-image web-image deps
	podman compose -f dev/podman-compose.yml up -d --force-recreate web api scs de-0 de-1 rte-0 rte-1 rte-2 re-0 re-1 ne plugin-std-random plugin-std-time plugin-std-artifact plugin-std-crypto plugin-std-dns plugin-std-http plugin-std-container scoc-1 scoc-2 scoc-3

down:
	podman compose -f dev/podman-compose.yml down

# ─── Two-region local harness ────────────────────────────────────────
#
# Brings up `loca` and `locb` end-to-end so cross-region machinery (token
# verify across edges, GDDB lookups, queue routing, DE cross-region
# dependency reads) can be exercised before any cloud deployment. SCOC
# and the container plugin are intentionally omitted; add them in
# dev/podman-compose.multi-region.yml when needed.

deps-multi-region:
	podman compose -f dev/podman-compose.multi-region.yml up -d \
		scylla-loca scylla-locb \
		rabbitmq-loca rabbitmq-locb \
		redis-loca redis-locb \
		redpanda-loca redpanda-locb \
		minio oci-registry buildkit mailhog
	@echo "Waiting for scylla-loca to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr-multi-region_scylla-loca_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for scylla-locb to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr-multi-region_scylla-locb_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq-loca to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr-multi-region_rabbitmq-loca_1 2>/dev/null)" != "healthy" ]; do sleep 2; done
	@echo "Waiting for rabbitmq-locb to become healthy..."
	@while [ "$$(podman inspect -f '{{.State.Health.Status}}' skyr-multi-region_rabbitmq-locb_1 2>/dev/null)" != "healthy" ]; do sleep 2; done

up-multi-region: image deps-multi-region
	podman compose -f dev/podman-compose.multi-region.yml up -d --force-recreate \
		api-loca scs-loca de-loca re-loca rte-loca ne-loca \
		plugin-std-random-loca plugin-std-artifact-loca plugin-std-crypto-loca \
		plugin-std-time-loca plugin-std-http-loca \
		api-locb scs-locb de-locb re-locb rte-locb ne-locb \
		plugin-std-random-locb plugin-std-artifact-locb plugin-std-crypto-locb \
		plugin-std-time-locb plugin-std-http-locb

down-multi-region:
	podman compose -f dev/podman-compose.multi-region.yml down

build-cli:
	cargo build --release -p cli

install-cli: build-cli
	sudo install target/release/skyr /usr/local/bin

uninstall-cli:
	sudo rm /usr/local/bin/skyr

cloud-config:
	envsubst < infra/scoc-cloud-config.yaml

# Compile the SCL formal specification to PDF. The PDF is a build artifact
# and should not be committed. Run from within `nix develop` to ensure the
# `typst` binary is on PATH.
spec:
	typst compile spec/main.typ spec/scl-spec.pdf

spec-watch:
	typst watch spec/main.typ spec/scl-spec.pdf

spec-clean:
	rm -f spec/scl-spec.pdf
