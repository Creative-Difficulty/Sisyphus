-- The desktop app's login waits on a confirmation page: a code is handed to
-- the app only after the user clicks "Log in" there, so nobody is linked by
-- following a login link by accident.
ALTER TABLE app_codes
    ADD COLUMN app_port  INTEGER,
    ADD COLUMN confirmed BOOLEAN NOT NULL DEFAULT FALSE;
