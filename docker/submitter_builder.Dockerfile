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

ENV NETWORK_RPC_URL https://eth-goerli.api.onfinality.io/public
ENV MAINNET_CHAIN_ID 5
ENV ORFeeManager_CONTRACT_ADDRESS 0xdf3CF1f661368036C694d50b3e9D85231f6fc7De
ENV TXS_SOURCE_URL https://openapi2.orbiter.finance/v3/yj6toqvwh1177e1sexfy0u1pxx5j8o47
ENV SUPPORT_CHAINS_SOURCE_URL https://api.studio.thegraph.com/query/49058/cabin/version/latest
ENV START_BLOCK 9755550
ENV ZK_DELAY_SECONDS 1800
ENV OP_DELAY_SECONDS 900
ENV COMMON_DELAY_SECONDS 450

USER submitter
EXPOSE 50001
VOLUME ["/data"]
