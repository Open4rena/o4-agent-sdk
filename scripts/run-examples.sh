#!/usr/bin/env bash
# Run every SDK example against a real o4-agent-runtime, with no provider
# account or API key.
#
#   scripts/run-examples.sh /path/to/o4-agent-runtime
#
# The examples use the runtime's built-in offline test model,
# open4rena-smoke:open4rena-usage-smoke. It answers fixed test prompts
# without contacting any provider, so the checks are of the SDK and runtime,
# not of a model's answers. Everything runs in a temporary directory with its
# own HOME; nothing else on the machine is read or changed.
#
# Needs python3 for the MCP example's server.

set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 /path/to/o4-agent-runtime" >&2
  exit 2
fi
runtime="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
[[ -x "$runtime" ]] || { echo "not an executable file: $1" >&2; exit 2; }
# An example reads input only where a check below supplies it.
exec </dev/null

repo="$(cd "$(dirname "$0")/.." && pwd)"
model="open4rena-smoke:open4rena-usage-smoke"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
# A command failing outside a check would end the script without a result;
# say where, so a stop is never mistaken for silence.
trap 'echo "run-examples.sh stopped unexpectedly at line $LINENO" >&2' ERR
mkdir -p "$scratch/home" "$scratch/tmp"

cargo build --quiet --locked --manifest-path "$repo/Cargo.toml" -p o4-agent-sdk --examples
bin="$(cargo metadata --locked --format-version 1 --no-deps --manifest-path "$repo/Cargo.toml" |
  python3 -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/examples"

failures=0
log="$scratch/output"

# Runs a command in its own process group, so the runtime it starts can be
# stopped with it. Exits 124 if the time limit passes, and 125 if the command
# exits but leaves a process running (the runtime should stop with it).
# shellcheck disable=SC2016 # The $ signs are Perl's, not the shell's.
supervise='
  my $seconds = shift;
  my $pid = fork() // die "cannot fork: $!\n";
  if ($pid == 0) {
    setpgrp(0, 0);
    exec @ARGV or die "cannot run $ARGV[0]: $!\n";
  }
  setpgrp($pid, $pid);
  my $stop = sub { kill "KILL", -$pid; waitpid($pid, 0); exit shift };
  $SIG{INT} = $SIG{TERM} = sub { $stop->(130) };
  $SIG{ALRM} = sub { print STDERR "timed out after ${seconds}s\n"; $stop->(124) };
  alarm $seconds;
  waitpid($pid, 0);
  my $status = $?;
  alarm 0;
  for (1 .. 30) { kill(0, -$pid) or last; select(undef, undef, undef, 0.1) }
  if (kill 0, -$pid) {
    kill "KILL", -$pid;
    print STDERR "the example exited but left a process running\n";
    exit 125;
  }
  exit($status & 127 ? 128 + ($status & 127) : $status >> 8);
'

# run <timeout-seconds> <directory> <example> [args...]: the output goes to
# $log. A minimal environment keeps API keys and settings from leaking in.
run() {
  local seconds="$1" dir="$2" example="$3"
  shift 3
  (cd "$dir" && env -i PATH="$PATH" HOME="$scratch/home" TMPDIR="$scratch/tmp" LANG=C.UTF-8 \
    perl -e "$supervise" "$seconds" "$bin/$example" "$runtime" "$@") >"$log" 2>&1
}

pass() { echo "ok      $1"; }
fail() {
  echo "FAILED  $1: $2"
  sed 's/^/        | /' "$log"
  failures=$((failures + 1))
}

# expect <name> <text>... : every text must appear in the output.
expect() {
  local name="$1"
  shift
  for text in "$@"; do
    if ! grep -qF -- "$text" "$log"; then
      fail "$name" "output lacks \"$text\""
      return
    fi
  done
  pass "$name"
}

workspace() {
  mktemp -d "$scratch/workspace.XXXXXX"
}

# --- one_shot --------------------------------------------------------------
dir="$(workspace)"
if run 60 "$dir" one_shot "$model" "hello"; then
  expect one_shot "final output: O4_PROVIDER_KEY_SMOKE_OK"
else
  fail one_shot "exited with status $?"
fi

# --- persistent: three turns, the last after closing and resuming ----------
dir="$(workspace)"
if run 60 "$dir" persistent "$model"; then
  expect persistent "turn 1: O4_PROVIDER_KEY_SMOKE_OK" "resumed turn: O4_PROVIDER_KEY_SMOKE_OK"
else
  fail persistent "exited with status $?"
fi

# expect_error <name> <text> <command...>: the example must report an error
# itself (exit status 1), not time out or leave a process running.
expect_error() {
  local name="$1" text="$2" status=0
  shift 2
  "$@" || status=$?
  if ((status == 1)); then
    expect "$name" "$text"
  else
    fail "$name" "exited with status $status, expected an error (1)"
  fi
}

# --- structured_report: the test model cannot fill in a schema -------------
dir="$(workspace)"
expect_error structured_report "no report: the run ended Failed" \
  run 60 "$dir" structured_report "$model" "$dir"

# --- live_progress ---------------------------------------------------------
dir="$(workspace)"
if run 60 "$dir" live_progress "$model" "hello"; then
  expect live_progress "O4_PROVIDER_KEY_SMOKE_OK" "run ended: Completed"
else
  fail live_progress "exited with status $?"
fi
# Enter right away. The test model answers instantly, so the run usually ends
# before the cancel request arrives. This checks that pressing Enter does not
# break the example; tests/e2e_runtime.rs checks a real cancel.
dir="$(workspace)"
if printf '\n' | run 60 "$dir" live_progress "$model" "hello"; then
  if grep -Eq '^run ended: (Completed|Cancelled)$' "$log"; then
    pass "live_progress (Enter during the run)"
  else
    fail "live_progress (Enter during the run)" "the run did not end completed or cancelled"
  fi
else
  fail "live_progress (Enter during the run)" "exited with status $?"
fi

# --- resume_after_restart: two separate processes, then a wrong directory --
dir="$(workspace)"
state="$scratch/state"
if run 60 "$dir" resume_after_restart "$model" "$state" "remember this"; then
  expect "resume_after_restart (first run)" "started conversation"
else
  fail "resume_after_restart (first run)" "exited with status $?"
fi
if run 60 "$dir" resume_after_restart "$model" "$state"; then
  expect "resume_after_restart (after restart)" "resuming conversation"
else
  fail "resume_after_restart (after restart)" "exited with status $?"
fi
elsewhere="$(workspace)"
expect_error "resume_after_restart (other directory)" "does not match" \
  run 60 "$elsewhere" resume_after_restart "$model" "$state"

# --- mcp_tools: the session starts the example MCP server ------------------
# The runtime starts the server in its own process group, which the
# supervisor does not watch, so check separately that it stopped: look for a
# process whose command line ends with this checkout's server path. Plain
# text comparison, so no path character is read as a pattern; nothing is
# stopped, only reported.
server="$repo/crates/o4-agent-sdk/examples/mcp/echo_server.py"
servers_running() {
  ps -A -o pid= -o command= | while read -r pid command; do
    if [[ "$command" == *" $server" ]]; then
      echo "$pid"
    fi
  done
}
before="$(servers_running)"
dir="$(workspace)"
if run 60 "$dir" mcp_tools "$model" "hello"; then
  expect mcp_tools "O4_PROVIDER_KEY_SMOKE_OK"
else
  fail mcp_tools "exited with status $?"
fi
sleep 1
leftover=""
for pid in $(servers_running); do
  # One already running before this check is someone else's.
  [[ " $before " == *" $pid "* ]] || leftover="$leftover $pid"
done
if [[ -n "$leftover" ]]; then
  : >"$log"
  fail "mcp_tools (server stopped)" "the MCP server kept running after the example exited (pid$leftover)"
else
  pass "mcp_tools (server stopped)"
fi

# --- human_approval: the test model's write waits for an answer ------------
# O4_EVAL_HARNESS_WRITE_SMOKE makes the test model write
# eval-harness-smoke.txt in the workspace.
#
# Known runtime bug (0.3.21 to 0.3.34): approval runs sometimes fail with
# "event sequence N is not greater than previous sequence M", because the
# runtime sends two events out of order (1 in 50 to 1 in 90 runs on the pinned
# 0.3.21, and 1 in 40 to 1 in 4 on 0.3.34). The SDK is right to stop. A run
# that fails with exactly that error is retried once, in a fresh workspace,
# and the retry is reported; any other failure, or a second one, counts.
runtime_order_bug='is not greater than previous sequence'
#
# approval <name> <file-expected: yes|no> <input> <text>...: run the example
# with <input> as the person's answers, then check whether the file was
# written and that every text appears in the output.
approval() {
  local name="$1" written="$2" input="$3"
  shift 3
  local dir status=0
  dir="$(workspace)"
  printf '%s' "$input" | run 60 "$dir" human_approval "$model" O4_EVAL_HARNESS_WRITE_SMOKE ||
    status=$?
  if ((status == 1)) && grep -qF -- "$runtime_order_bug" "$log"; then
    echo "retry   $name: the runtime sent events out of order (known runtime bug); running it again"
    status=0
    dir="$(workspace)"
    printf '%s' "$input" | run 60 "$dir" human_approval "$model" O4_EVAL_HARNESS_WRITE_SMOKE ||
      status=$?
  fi
  if ((status != 0)); then
    fail "$name" "exited with status $status"
  elif [[ "$written" == yes && ! -f "$dir/eval-harness-smoke.txt" ]]; then
    fail "$name" "the approved write did not happen"
  elif [[ "$written" == no && -e "$dir/eval-harness-smoke.txt" ]]; then
    fail "$name" "the file was written without approval"
  else
    expect "$name" "$@"
  fi
}
approval "human_approval (approved)" yes $'y\n' "Allow \"write\" on" "O4_EVAL_HARNESS_WRITE_SMOKE_OK"
approval "human_approval (refused)" no $'n\n' "Allow \"write\" on" "O4_EVAL_HARNESS_WRITE_SMOKE_FAILED"
approval "human_approval (no input)" no "" "refused \"write\" on" "O4_EVAL_HARNESS_WRITE_SMOKE_FAILED"

if ((failures > 0)); then
  echo "$failures example check(s) failed"
  exit 1
fi
echo "all example checks passed"
