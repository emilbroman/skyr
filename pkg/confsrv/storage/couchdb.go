package storage

import (
	"context"
	"log/slog"
	"strings"

	"github.com/go-git/go-git/v5/plumbing/storer"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-kivik/kivik/v4"
	"github.com/redis/go-redis/v9"
)

func createDatabaseIfNotExists(c *kivik.Client, name string) (*kivik.DB, error) {
	err := c.CreateDB(context.Background(), name)

	if err != nil && !strings.Contains(err.Error(), "already exists") {
		return nil, err
	}

	return c.DB(name), nil
}

type CouchDBRepoLoader struct {
	ctx      context.Context
	dbClient *kivik.Client
	rdb      *redis.Client
}

func NewCouchDBRepoLoader(ctx context.Context, dbClient *kivik.Client, rdb *redis.Client) CouchDBRepoLoader {
	return CouchDBRepoLoader{
		ctx,
		dbClient,
		rdb,
	}
}

func (c CouchDBRepoLoader) Load(ep *transport.Endpoint) (storer.Storer, error) {
	slog.Debug("opening repo", slog.String("path", ep.Path), slog.String("user", ep.User))

	repo := ep.Path[1:]

	refsRepo, err := NewRefsRepo(c.ctx, c.dbClient, repo)
	if err != nil {
		return nil, err
	}

	return NewStorer(c.ctx,
		refsRepo,
		NewObjsRepo(c.rdb, repo),
	), nil
}
