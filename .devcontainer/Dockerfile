FROM espressif/idf-rust:all_latest

USER root
RUN apt-get update \
    && apt-get install -y --no-install-recommends libudev-dev \
    && rm -rf /var/lib/apt/lists/*
USER esp

# Ensure toolchain env is sourced for interactive shells
RUN echo '. /home/esp/export-esp.sh' >> /home/esp/.bashrc

COPY entrypoint.sh /home/esp/entrypoint.sh
USER root
RUN chmod +x /home/esp/entrypoint.sh
USER esp

# Pre-create cargo cache dirs so Docker named volumes inherit esp ownership
RUN mkdir -p /home/esp/.cargo/registry /home/esp/.cargo/git

ENTRYPOINT ["/home/esp/entrypoint.sh"]
CMD ["bash"]
