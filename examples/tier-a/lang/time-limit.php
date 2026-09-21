<?php
// The execution timer (ADR-036): the fatal at the back edge, its location and trace, and the shutdown function that still runs — both engines exit 255.
register_shutdown_function(function () { echo "shutdown ran\n"; });
set_time_limit(1);
echo "start\n";
while (true) {}
