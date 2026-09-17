<?php
// intdiv() by zero throws DivisionByZeroError ("Division by zero"); exit 255.
// Nothing is echoed before the fault on purpose (see divergences.toml).
echo intdiv(1, 0);
