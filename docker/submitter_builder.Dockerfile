# This is the build stage for Substrate. Here we create the binary.
FROM docker.io/paritytech/ci-linux:production as builder

WORKDIR /submitter
COPY . /submitter
RUN cargo build --release

# This is the 2nd stage: a very small image where we copy the submitter binary."
FROM docker.io/library/ubuntu:20.04

COPY --from=builder /submitter/target/release/submitter /usr/local/bin

RUN useradd -m -u 1000 -U -s /bin/sh -d /submitter submitter && \
	mkdir -p /data /submitter/db && \
	chown -R submitter:submitter /data && \
	ln -s /data /submitter/db && \
# Sanity checks
	ldd /usr/local/bin/submitter && \
# unclutter and minimize the attack surface
	rm -rf /usr/bin /usr/sbin && \
	/usr/local/bin/submitter --version

USER submitter
EXPOSE 50001
VOLUME ["/data"]
