-- name: InsertRoom :exec
insert into rendezvous_rooms
    (id, created_by, expires_at)
values($1,$2,$3);