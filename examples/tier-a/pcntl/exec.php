<?php
// fork + pcntl_exec: the child becomes another program (argument and
// environment conversion included); the parent reaps it.
function run(string $path, array $args, ?array $env = null): void
{
    $pid = pcntl_fork();
    if ($pid === 0) {
        if ($env === null) {
            pcntl_exec($path, $args);
        } else {
            pcntl_exec($path, $args, $env);
        }
        echo "exec failed: ", pcntl_get_last_error(), "\n";
        exit(99);
    }
    pcntl_waitpid($pid, $status);
    echo "-> exit ", pcntl_wexitstatus($status), "\n";
}

run('/bin/echo', ['hello', 'from', 'echo']);
run('/bin/echo', ['a', 5, 1.5, true, null, 'z']);
run('/usr/bin/env', [], ['A' => '1', 5 => 'B=2', 'C' => 3]);
run('/bin/sh', ['-c', 'exit 4']);
run('/no/such/program', []);
