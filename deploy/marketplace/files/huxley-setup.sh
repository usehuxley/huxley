# Offers Huxley's setup on an interactive root login until it's done.
if [ "$(id -u)" = 0 ] && [ -t 0 ] && [ ! -f /opt/huxley/deploy/compose/.configured ]; then
	echo "Run 'huxley-setup' to set Huxley up."
fi
