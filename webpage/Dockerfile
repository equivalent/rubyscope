# The web page: plain HTML and CSS, served by nginx. Built by `kamal deploy` on
# Linux, never on macOS.
FROM nginx:1.30.7-alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY . /usr/share/nginx/html/
RUN rm /usr/share/nginx/html/Dockerfile /usr/share/nginx/html/nginx.conf
