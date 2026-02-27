#!/bin/sh
set -e

REDIS_HOST="${REDIS_HOST:-redis}"
REDIS_PORT="${REDIS_PORT:-6379}"

until valkey-cli -h "$REDIS_HOST" -p "$REDIS_PORT" ping 2>/dev/null | grep -q PONG; do
  echo "Waiting for Valkey..."
  sleep 1
done

echo "Connected to Valkey at $REDIS_HOST:$REDIS_PORT"
echo "Publishing random FX rates every 500ms..."
echo ""

# Long-running awk process maintains random-walk state across ticks
# and shells out to redis-cli to publish each rate update.
awk -v host="$REDIS_HOST" -v port="$REDIS_PORT" '
BEGIN {
  srand()

  eurusd = 1.08000
  gbpusd = 1.27000
  usdjpy = 150.000
  audusd = 0.65000

  cli = "valkey-cli -h " host " -p " port " PUBLISH "
  null = " >/dev/null"

  for (tick = 1; ; tick++) {
    # Random walk with mean reversion toward base rates
    eurusd += (rand() - 0.5) * 0.0004 + (1.08000 - eurusd) * 0.01
    gbpusd += (rand() - 0.5) * 0.0004 + (1.27000 - gbpusd) * 0.01
    usdjpy += (rand() - 0.5) * 0.04   + (150.000 - usdjpy) * 0.01
    audusd += (rand() - 0.5) * 0.0004 + (0.65000 - audusd) * 0.01

    # Publish bid and ask for each pair (small market spread around mid)
    system(cli "fx:EURUSD:bid " sprintf("%.5f", eurusd - 0.00005) null)
    system(cli "fx:EURUSD:ask " sprintf("%.5f", eurusd + 0.00005) null)
    system(cli "fx:GBPUSD:bid " sprintf("%.5f", gbpusd - 0.00005) null)
    system(cli "fx:GBPUSD:ask " sprintf("%.5f", gbpusd + 0.00005) null)
    system(cli "fx:USDJPY:bid " sprintf("%.3f", usdjpy - 0.005)   null)
    system(cli "fx:USDJPY:ask " sprintf("%.3f", usdjpy + 0.005)   null)
    system(cli "fx:AUDUSD:bid " sprintf("%.5f", audusd - 0.00005) null)
    system(cli "fx:AUDUSD:ask " sprintf("%.5f", audusd + 0.00005) null)

    if (tick % 20 == 0)
      printf "[tick %d] EURUSD=%.5f GBPUSD=%.5f USDJPY=%.3f AUDUSD=%.5f\n", \
        tick, eurusd, gbpusd, usdjpy, audusd

    system("sleep 0.5")
  }
}
' </dev/null
