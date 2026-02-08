package storage

import (
	"context"
	"errors"
	"fmt"

	"github.com/go-git/go-git/v5/plumbing"
	"github.com/redis/go-redis/v9"
)

type ObjsRepo struct {
	rdb  *redis.Client
	repo string
}

func NewObjsRepo(rdb *redis.Client, repo string) *ObjsRepo {
	return &ObjsRepo{rdb, repo}
}

func (r *ObjsRepo) Has(ctx context.Context, t plumbing.ObjectType, h plumbing.Hash) (bool, error) {
	cmd := r.rdb.Exists(ctx, fmt.Sprintf("%s:%d:%s", r.repo, t, h))

	if cmd.Err() != nil {
		return false, cmd.Err()
	}

	i, err := cmd.Result()
	return i == 1, err
}

func (r *ObjsRepo) Get(ctx context.Context, t plumbing.ObjectType, h plumbing.Hash) (plumbing.EncodedObject, error) {
	cmd := r.rdb.Get(ctx, fmt.Sprintf("%s:%d:%s", r.repo, t, h))

	if cmd.Err() != nil {
		return nil, cmd.Err()
	}

	blob, err := cmd.Bytes()
	if err != nil {
		return nil, err
	}

	var obj plumbing.MemoryObject
	obj.SetSize(int64(len(blob)))
	obj.SetType(t)
	_, err = obj.Write(blob)

	if err != nil {
		return nil, err
	}

	return &obj, nil
}

func (r *ObjsRepo) Put(ctx context.Context, t plumbing.ObjectType, h plumbing.Hash, blob []byte) error {
	if t == plumbing.AnyObject {
		return errors.New("cannot put anyobject")
	}

	cmd := r.rdb.MSet(ctx, map[string]any{
		fmt.Sprintf("%s:%d:%s", r.repo, t, h):                  blob,
		fmt.Sprintf("%s:%d:%s", r.repo, plumbing.AnyObject, h): blob,
	})

	return cmd.Err()
}
