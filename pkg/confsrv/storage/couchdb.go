package storage

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"strings"

	"github.com/emilbroman/skyr/pkg/confsrv"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/storer"
	"github.com/go-git/go-git/v5/plumbing/transport"
	"github.com/go-kivik/kivik/v4"
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
}

func NewCouchDBRepoLoader(ctx context.Context, dbClient *kivik.Client) CouchDBRepoLoader {
	return CouchDBRepoLoader{
		ctx,
		dbClient,
	}
}

func (c CouchDBRepoLoader) Load(ep *transport.Endpoint) (storer.Storer, error) {
	slog.Debug("opening repo", slog.String("path", ep.Path), slog.String("user", ep.User))

	objDB, err := createDatabaseIfNotExists(c.dbClient, fmt.Sprintf("repo/objs%s", ep.Path))
	if err != nil {
		return nil, err
	}

	refDB, err := createDatabaseIfNotExists(c.dbClient, fmt.Sprintf("repo/refs%s", ep.Path))
	if err != nil {
		return nil, err
	}

	return NewCouchDBStorer(c.ctx, objDB, refDB, ep)
}

type CouchDBStorer struct {
	ctx context.Context

	objDB *kivik.DB
	refs  *RefsRepo

	ep *transport.Endpoint
}

func NewCouchDBStorer(ctx context.Context, objDB *kivik.DB, refDB *kivik.DB, ep *transport.Endpoint) (*CouchDBStorer, error) {
	refs, err := NewRefsRepo(ctx, refDB)
	if err != nil {
		return nil, err
	}
	return &CouchDBStorer{ctx, objDB, refs, ep}, nil
}

type scanable interface {
	Err() error
	ScanDoc(any) error
}

func decodeDoc[T any](doc scanable) (*T, error) {
	if doc.Err() != nil {
		if strings.Contains(doc.Err().Error(), "Not Found") {
			return nil, nil
		}

		return nil, doc.Err()
	}

	var d T
	err := doc.ScanDoc(&d)
	if err != nil {
		return nil, err
	}

	return &d, nil
}

// CheckAndSetReference implements [storer.ReferenceStorer].
func (c *CouchDBStorer) CheckAndSetReference(new *plumbing.Reference, old *plumbing.Reference) error {
	if old.Name() != new.Name() {
		return fmt.Errorf("cannot set %v over %v", new.Name(), old.Name())
	}
	slog.Debug("check and set reference")

	if old != nil {
		existingDoc, err := c.refs.Get(c.ctx, new.Name())
		if err != nil {
			return err
		}
		if existingDoc == nil {
			return plumbing.ErrReferenceNotFound
		}

		existing := existingDoc.ToReference()
		if old.Hash() != existing.Hash() {
			return fmt.Errorf("%v is not up-to-date", old.Name())
		}
	}

	_, err := c.refs.Set(c.ctx, new)
	slog.Debug("set reference", slog.Any("err", err))

	return err
}

// CountLooseRefs implements [storer.ReferenceStorer].
func (c CouchDBStorer) CountLooseRefs() (int, error) {
	return 0, nil
}

// IterReferences implements [storer.ReferenceStorer].
func (c CouchDBStorer) IterReferences() (storer.ReferenceIter, error) {
	return c.refs.ListCurrent(c.ctx)
}

// PackRefs implements [storer.ReferenceStorer].
func (c CouchDBStorer) PackRefs() error {
	slog.Error("ref packing not implemented")
	return nil
}

// Reference implements [storer.ReferenceStorer].
func (c CouchDBStorer) Reference(name plumbing.ReferenceName) (*plumbing.Reference, error) {
	doc, err := c.refs.Get(c.ctx, name)
	slog.Debug("reference doc", slog.String("name", name.String()), slog.Any("doc", doc), slog.Any("err", err))
	if err != nil {
		return nil, err
	}
	if doc == nil || doc.Action == confsrv.RefActionUnset {
		return nil, plumbing.ErrReferenceNotFound
	}
	return doc.ToReference(), nil
}

// RemoveReference implements [storer.ReferenceStorer].
func (c CouchDBStorer) RemoveReference(name plumbing.ReferenceName) error {
	slog.Debug("removereference", slog.String("name", name.String()))
	ref, err := c.Reference(name)
	if err != nil {
		return err
	}
	_, err = c.refs.Unset(c.ctx, ref)
	return err
}

// SetReference implements [storer.ReferenceStorer].
func (c CouchDBStorer) SetReference(ref *plumbing.Reference) error {
	slog.Debug("will set reference")
	_, err := c.refs.Set(c.ctx, ref)
	slog.Debug("set reference", slog.Any("err", err))
	return err
}

// AddAlternate implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) AddAlternate(remote string) error {
	return errors.ErrUnsupported
}

// EncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) EncodedObject(t plumbing.ObjectType, h plumbing.Hash) (out plumbing.EncodedObject, err error) {
	d, err := decodeDoc[confsrv.ObjDoc](c.objDB.Get(c.ctx, h.String()))
	if err != nil {
		return nil, err
	}
	if t != plumbing.AnyObject && d.Type() != t {
		return nil, plumbing.ErrInvalidType
	}
	return d, nil
}

// EncodedObjectSize implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) EncodedObjectSize(h plumbing.Hash) (int64, error) {
	d, err := decodeDoc[confsrv.ObjDoc](c.objDB.Get(c.ctx, h.String()))
	if err != nil {
		return 0, err
	}
	return d.Size(), nil
}

// HasEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) HasEncodedObject(h plumbing.Hash) error {
	_, err := c.objDB.GetRev(c.ctx, h.String())
	if err != nil {
		return err
	}
	return nil
}

// IterEncodedObjects implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) IterEncodedObjects(t plumbing.ObjectType) (storer.EncodedObjectIter, error) {
	set := c.objDB.Find(c.ctx, map[string]any{"selector": map[string]any{
		"type": t,
	}})

	return couchObjIter{set}, nil
}

// NewEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) NewEncodedObject() plumbing.EncodedObject {
	return &confsrv.ObjDoc{}
}

// SetEncodedObject implements [storer.EncodedObjectStorer].
func (c CouchDBStorer) SetEncodedObject(o plumbing.EncodedObject) (plumbing.Hash, error) {
	slog.Info("setting obj", slog.String("ep", c.ep.String()), slog.String("ot", o.Type().String()), slog.String("h", o.Hash().String()))
	h := o.Hash()

	docID := h.String()
	rev, err := c.objDB.GetRev(c.ctx, docID)
	if err != nil && !strings.Contains(err.Error(), "Not Found") {
		return h, err
	}

	r, err := o.Reader()
	if err != nil {
		return h, err
	}
	defer r.Close()
	data, err := io.ReadAll(r)

	doc := &confsrv.ObjDoc{
		ID:  h.String(),
		Rev: rev,
		Obj: confsrv.ObjBlob{
			Type: o.Type(),
			Size: o.Size(),
			Data: data,
		},
	}

	_, err = c.objDB.Put(c.ctx, docID, doc)

	return h, err
}

type couchObjIter struct {
	set *kivik.ResultSet
}

// Close implements [storer.EncodedObjectIter].
func (c couchObjIter) Close() {
	if err := c.set.Close(); err != nil {
		slog.Error("error while closing result set", slog.Any("err", err))
	}
}

// ForEach implements [storer.EncodedObjectIter].
func (c couchObjIter) ForEach(f func(plumbing.EncodedObject) error) error {
	for {
		doc, err := decodeDoc[confsrv.ObjDoc](c.set)
		if err != nil {
			c.set.Close()
			return err
		}

		err = f(doc)
		if err != nil {
			c.set.Close()
			return err
		}

		if !c.set.Next() {
			return nil
		}
	}
}

// Next implements [storer.EncodedObjectIter].
func (c couchObjIter) Next() (plumbing.EncodedObject, error) {
	if !c.set.Next() {
		return nil, nil
	}

	doc, err := decodeDoc[confsrv.ObjDoc](c.set)
	if err != nil {
		c.set.Close()
		return nil, err
	}

	return doc, nil
}
