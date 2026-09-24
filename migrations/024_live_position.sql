-- Owner switch for showing a car's live position to the people it is shared with.
-- Sharing a car's trip history and broadcasting where it is right now are different
-- things; the owner always sees it, sharees only while this is on.
ALTER TABLE cars
    ADD COLUMN IF NOT EXISTS share_live_position BOOLEAN NOT NULL DEFAULT true;
