#!/bin/sh
set -e

SERVER="http://fx-pricing:3000"

echo "Waiting for FX pricing server..."
until curl -sf "$SERVER/trades" >/dev/null 2>&1; do
  sleep 2
done

echo "Server is up. Creating sample trades..."
echo ""

curl -s -X POST "$SERVER/trades" \
  -H "Content-Type: application/json" \
  -d '{"pair":"EURUSD","spread_bps":2,"client":"Hedge Fund Alpha"}'
echo ""

curl -s -X POST "$SERVER/trades" \
  -H "Content-Type: application/json" \
  -d '{"pair":"GBPUSD","spread_bps":3,"client":"Pension Fund Beta"}'
echo ""

curl -s -X POST "$SERVER/trades" \
  -H "Content-Type: application/json" \
  -d '{"pair":"USDJPY","spread_bps":1.5,"client":"Bank Gamma"}'
echo ""

curl -s -X POST "$SERVER/trades" \
  -H "Content-Type: application/json" \
  -d '{"pair":"AUDUSD","spread_bps":4,"client":"Retail Delta"}'
echo ""

echo ""
echo "Created 4 sample trades. Prices are now streaming."
