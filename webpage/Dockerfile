# The web page: plain HTML and CSS, served by nginx. Built by `kamal deploy` on
# Linux, never on macOS. .dockerignore lets in only the files below.
FROM nginx:1.30.7-alpine
RUN rm -rf /usr/share/nginx/html/* /etc/nginx/conf.d/*
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY index.html favicon.ico /usr/share/nginx/html/
COPY css /usr/share/nginx/html/css
COPY docs /usr/share/nginx/html/docs
COPY fonts /usr/share/nginx/html/fonts
COPY img /usr/share/nginx/html/img
COPY js /usr/share/nginx/html/js
