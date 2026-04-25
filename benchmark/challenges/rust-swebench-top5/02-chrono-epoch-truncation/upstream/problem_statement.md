Truncating a timestamp close to EPOCH fails
Due to this check in `duration_truc`:
https://github.com/chronotope/chrono/blob/main/src/round.rs#L221-L223

Expected behaviour: I should be able to truncate the timestamp to 0
