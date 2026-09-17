<?php
// `%` by zero throws DivisionByZeroError ("Modulo by zero"); exit code 255.
// Nothing is echoed before the fault on purpose (see divergences.toml).
echo 1 % 0;
