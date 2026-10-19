FROM alpine:3

RUN printf '#!/bin/sh\n(sleep 1; touch /tmp/service-ready) &\nexec "$@"\n' > /entrypoint.sh \
    && chmod +x /entrypoint.sh

ENTRYPOINT ["/entrypoint.sh"]
