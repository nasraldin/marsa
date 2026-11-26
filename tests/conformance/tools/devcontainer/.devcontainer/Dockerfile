FROM busybox@sha256:a7dbc319b1438115a207e11930dc4bb3e643dcb3ec0bdd694bb36cd8fad289ec
# The Dev Containers CLI reads os-release while preparing the remote environment.
RUN printf 'NAME=BusyBox\nID=busybox\n' > /etc/os-release && echo marsa-build > /image-marker
