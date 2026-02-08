package storage

import (
	"context"
	_ "embed"
	"fmt"
	"log/slog"

	"github.com/emilbroman/skyr/pkg/confsrv"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/storer"
	"github.com/go-kivik/kivik/v4"
)

type RefsRepo struct {
	db *kivik.DB
}

//go:embed latest_by_name_map.js
var latestByNameMap string

//go:embed latest_by_name_reduce.js
var latestByNameReduce string

type latestByNameRecord struct {
	Timestamp      int                    `json:"timestamp"`
	DocID          string                 `json:"doc_id"`
	Name           plumbing.ReferenceName `json:"name"`
	Hash           string                 `json:"hash,omitempty"`
	SymbolicTarget plumbing.ReferenceName `json:"symbolic_target,omitempty"`
	Rollout        confsrv.Rollout        `json:"rollout"`
}

//go:embed by_name_map.js
var byNameMap string

type byNameRecord struct {
	Timestamp int    `json:"timestamp"`
	DocID     string `json:"doc_id"`
}

func NewRefsRepo(ctx context.Context, client *kivik.Client, repo string) (*RefsRepo, error) {
	db, err := createDatabaseIfNotExists(client, fmt.Sprintf("refs/%s", repo))
	if err != nil {
		return nil, err
	}

	err = PutDesignDoc(ctx, db, "names", map[string]View{
		"latest_by_name": {
			Map:    latestByNameMap,
			Reduce: latestByNameReduce,
		},
		"by_name": {
			Map: byNameMap,
		},
	})
	if err != nil {
		return nil, err
	}
	return &RefsRepo{db}, nil
}

func (r *RefsRepo) RefHistory(ctx context.Context, name plumbing.ReferenceName, ch chan<- *confsrv.RefDoc) error {
	defer close(ch)
	rs := r.db.Query(ctx, "names", "by_name", kivik.Param("key", name.String()), kivik.Param("descending", "true"))
	for rs.Next() {
		var rec byNameRecord
		err := rs.ScanValue(&rec)
		if err != nil {
			rs.Close()
			return err
		}

		d := r.db.Get(ctx, rec.DocID)
		if d.Err() != nil {
			rs.Close()
			return d.Err()
		}

		var rd confsrv.RefDoc
		err = d.ScanDoc(&rd)
		if err != nil {
			rs.Close()
			return err
		}

		ch <- &rd
	}
	return nil
}

func (r *RefsRepo) ListDesired(ctx context.Context) (storer.ReferenceIter, error) {
	slog.Debug("iterating references")

	rs := r.db.Query(ctx, "names", "latest_by_name", kivik.Param("group", true))

	if rs.Err() != nil {
		return nil, rs.Err()
	}

	filter := func(r *latestByNameRecord) bool {
		return r.Rollout == confsrv.RolloutDesired || r.Rollout == confsrv.RolloutUp
	}

	return resultSetRefIter{rs, filter}, nil
}

func (r *RefsRepo) ListAll(ctx context.Context) (storer.ReferenceIter, error) {
	rs := r.db.Query(ctx, "names", "latest_by_name", kivik.Param("group", true))

	if rs.Err() != nil {
		return nil, rs.Err()
	}

	filter := func(r *latestByNameRecord) bool {
		return true
	}

	return resultSetRefIter{rs, filter}, nil
}

func (r *RefsRepo) Get(ctx context.Context, name plumbing.ReferenceName) (*confsrv.RefDoc, error) {
	rs := r.db.Query(ctx, "names", "latest_by_name", kivik.Param("key", name))
	if !rs.Next() {
		return nil, nil
	}

	var doc latestByNameRecord
	err := rs.ScanValue(&doc)
	if err != nil {
		return nil, fmt.Errorf("failed to scan view doc: %w", err)
	}

	dbdoc := r.db.Get(ctx, doc.DocID)
	if dbdoc.Err() != nil {
		return nil, fmt.Errorf("failed to hydrate view doc: %w", dbdoc.Err())
	}

	var refdoc confsrv.RefDoc
	err = dbdoc.ScanDoc(&refdoc)
	if err != nil {
		return nil, fmt.Errorf("failed to scan ref doc: %w", err)
	}

	return &refdoc, nil
}

func (r *RefsRepo) Set(ctx context.Context, ref *plumbing.Reference) (*confsrv.RefDoc, error) {
	doc := confsrv.CreateRefDoc(ref)

	prev, err := r.Get(ctx, ref.Name())
	if err != nil {
		return nil, fmt.Errorf("failed to lookup previous ref: %w", err)
	}

	if prev != nil {
		prev.Rollout = confsrv.RolloutLinger
		prev.SupercededBy = doc.ID
		prev.Rev, err = r.db.Put(ctx, prev.ID, prev)
		if err != nil {
			return nil, fmt.Errorf("failed to supercede prev: %w", err)
		}
	}

	rev, err := r.db.Put(ctx, doc.ID, doc)
	if err != nil {
		return nil, fmt.Errorf("failed to set ref: %w", err)
	}
	doc.Rev = rev

	return doc, nil
}

func (r *RefsRepo) Unset(ctx context.Context, ref *plumbing.Reference) (*confsrv.RefDoc, error) {
	doc, err := r.Get(ctx, ref.Name())
	if err != nil {
		return nil, fmt.Errorf("failed to lookup previous ref: %w", err)
	}

	if doc == nil {
		return nil, nil
	}

	doc.Rollout = confsrv.RolloutUndesired

	doc.Rev, err = r.db.Put(ctx, doc.ID, doc)
	if err != nil {
		return nil, fmt.Errorf("failed to update prev: %w", err)
	}

	return doc, nil
}

type resultSetRefIter struct {
	rs     *kivik.ResultSet
	filter func(*latestByNameRecord) bool
}

// Close implements [storer.ReferenceIter].
func (r resultSetRefIter) Close() {
	slog.Debug("closing reference iter")
	err := r.rs.Close()
	if err != nil {
		slog.Error("error when closing result set", slog.Any("err", err))
		return
	}
	slog.Debug("closed reference iter")
}

// ForEach implements [storer.ReferenceIter].
func (r resultSetRefIter) ForEach(f func(*plumbing.Reference) error) error {
	for ref, err := r.Next(); ref != nil; ref, err = r.Next() {
		if err != nil {
			return err
		}

		err = f(ref)
		if err != nil {
			return err
		}
	}

	slog.Debug("reference foreach ended")

	return nil
}

// Next implements [storer.ReferenceIter].
func (r resultSetRefIter) Next() (*plumbing.Reference, error) {
	if !r.rs.Next() {
		slog.Debug("reference iter ended")
		return nil, nil
	}

	var doc latestByNameRecord
	err := r.rs.ScanValue(&doc)
	if err != nil {
		return nil, fmt.Errorf("failed to scan reference iter doc: %w", err)
	}

	if !r.filter(&doc) {
		slog.Debug("excluding doc", slog.Any("doc", doc))
		return r.Next()
	}

	slog.Debug("ref iter doc", slog.Any("doc", doc))

	if doc.Hash != "" {
		return plumbing.NewHashReference(doc.Name, plumbing.NewHash(doc.Hash)), nil
	}

	return plumbing.NewSymbolicReference(doc.Name, doc.SymbolicTarget), nil
}
