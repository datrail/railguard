# check=skip=InvalidDefaultArgInFrom
# A WireMock stub with its mappings baked in, since Cloud Run mounts no local
# folder. The build context is the mappings folder; session.sh passes the
# image its stack's compose.yml pins.
# No default, so the pin stays in compose.yml alone.
ARG WIREMOCK
FROM ${WIREMOCK}
COPY . /home/wiremock/mappings/
