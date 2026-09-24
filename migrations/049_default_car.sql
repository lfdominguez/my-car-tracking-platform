-- The car the web app preselects in its filters and on the live map.
--
-- May be a car shared with the user. Deleting the car clears it; losing a share
-- does not, so /api/me only reports it while the user can still read the car.
ALTER TABLE users
    ADD COLUMN IF NOT EXISTS default_car_id UUID REFERENCES cars(id) ON DELETE SET NULL;
