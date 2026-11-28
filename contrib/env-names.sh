# env-names.sh: sourced by contrib/slot-wrapper. Forwards the job's whole environment to the node
# (SLOT_ENV), except variables that describe this machine's session. The client itself keeps
# credential-like names back unless SLOT_ENV names them; this list never adds such names.
SLOT_ENV=$(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p' | \
  grep -Ev '^(PATH|HOME|PWD|OLDPWD|SHLVL|_|SHELL|USER|LOGNAME|MAIL|TMPDIR|TMP|TEMP|HOSTNAME|DISPLAY|COLORTERM|TERM|TERM_PROGRAM.*|LS_COLORS|XDG_.*|SSH_.*|WAYLAND_.*|DBUS_.*|SLOT_.*|.*(TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|_KEY|AUTH).*)$' | \
  sort -u | paste -sd, -)
export SLOT_ENV
