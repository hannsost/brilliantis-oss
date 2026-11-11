#!/usr/bin/env bash
# Optional document worker. The Rust application does not need Java.
# Input PDF must already be PDF/A and correspond to the same immutable invoice.
set -euo pipefail
umask 077
if [[ $# != 4 ]]; then
  echo 'Usage: zugferd.sh Mustang-CLI.jar invoice.pdfa.pdf factur-x.xml output.pdf' >&2
  exit 2
fi
brilliantis_jar=$(realpath -- "$1")
brilliantis_pdf=$(realpath -- "$2")
brilliantis_xml=$(realpath -- "$3")
brilliantis_output=$(realpath -m -- "$4")
[[ -f "$brilliantis_jar" && -f "$brilliantis_pdf" && -f "$brilliantis_xml" ]]
if [[ -e "$brilliantis_output" ]]; then echo 'Output exists; select a new destination.' >&2; exit 2; fi
brilliantis_stage=$(mktemp -d -- "$(dirname -- "$brilliantis_output")/.brilliantis-zugferd.XXXXXXXX")
trap 'rm -rf -- "$brilliantis_stage"' EXIT
# Record full validation reports privately; never suppress arithmetic or PDF/A checks.
timeout 120s java -jar "$brilliantis_jar" --action validate --source "$brilliantis_xml" --disable-file-logging > "$brilliantis_stage/xml-validation.xml" 2> "$brilliantis_stage/worker.log"
timeout 120s java -jar "$brilliantis_jar" --action combine --source "$brilliantis_pdf" --source-xml "$brilliantis_xml" --out "$brilliantis_stage/invoice.pdf" --format zf --version 2 --profile E --attachments '' --disable-file-logging >> "$brilliantis_stage/worker.log" 2>&1
timeout 120s java -jar "$brilliantis_jar" --action validate --source "$brilliantis_stage/invoice.pdf" --disable-file-logging > "$brilliantis_stage/pdf-validation.xml" 2>> "$brilliantis_stage/worker.log"
# Atomic no-clobber publication, only after the validator exits successfully.
ln -- "$brilliantis_stage/invoice.pdf" "$brilliantis_output"
ln -- "$brilliantis_stage/xml-validation.xml" "${brilliantis_output}.xml-validation.xml"
ln -- "$brilliantis_stage/pdf-validation.xml" "${brilliantis_output}.pdf-validation.xml"
sha256sum -- "$brilliantis_output"
