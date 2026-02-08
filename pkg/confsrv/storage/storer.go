package storage

import (
	"context"
	"errors"
	"fmt"
	"io"
	"log/slog"

	"github.com/emilbroman/skyr/pkg/confsrv"
	"github.com/go-git/go-git/v5/plumbing"
	"github.com/go-git/go-git/v5/plumbing/storer"
)

type Storer struct {
	storer.ReferenceStorer
	storer.EncodedObjectStorer
}

func NewStorer(ctx context.Context, refs *RefsRepo, objs *ObjsRepo) storer.Storer {
	refsStorer := NewReferenceStorer(ctx, refs)
	objsStorer := NewEncodedObjectStorer(ctx, objs)
	return &Storer{refsStorer, objsStorer}
}

type refsStorer struct {
	ctx context.Context
	r   *RefsRepo
}

func NewReferenceStorer(ctx context.Context, r *RefsRepo) storer.ReferenceStorer {
	return &refsStorer{ctx, r}
}

// CheckAndSetReference implements [storer.ReferenceStorer].
func (r *refsStorer) CheckAndSetReference(new *plumbing.Reference, old *plumbing.Reference) error {
	if old.Name() != new.Name() {
		return fmt.Errorf("cannot set %v over %v", new.Name(), old.Name())
	}
	slog.Debug("check and set reference")

	if old != nil {
		existingDoc, err := r.r.Get(r.ctx, new.Name())
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

	_, err := r.r.Set(r.ctx, new)
	slog.Debug("set reference", slog.Any("err", err))

	return err
}

// CountLooseRefs implements [storer.ReferenceStorer].
func (r *refsStorer) CountLooseRefs() (int, error) {
	return 0, nil
}

// IterReferences implements [storer.ReferenceStorer].
func (r *refsStorer) IterReferences() (storer.ReferenceIter, error) {
	return r.r.ListDesired(r.ctx)
}

// PackRefs implements [storer.ReferenceStorer].
func (r *refsStorer) PackRefs() error {
	return errors.ErrUnsupported
}

// Reference implements [storer.ReferenceStorer].
func (r *refsStorer) Reference(name plumbing.ReferenceName) (*plumbing.Reference, error) {
	doc, err := r.r.Get(r.ctx, name)
	slog.Debug("reference doc", slog.String("name", name.String()), slog.Any("doc", doc), slog.Any("err", err))
	if err != nil {
		return nil, err
	}
	if doc == nil || doc.Rollout == confsrv.RolloutUndesired {
		return nil, plumbing.ErrReferenceNotFound
	}
	return doc.ToReference(), nil
}

// RemoveReference implements [storer.ReferenceStorer].
func (r *refsStorer) RemoveReference(name plumbing.ReferenceName) error {
	slog.Debug("removereference", slog.String("name", name.String()))
	ref, err := r.Reference(name)
	if err != nil {
		return err
	}
	_, err = r.r.Unset(r.ctx, ref)
	return err
}

// SetReference implements [storer.ReferenceStorer].
func (r *refsStorer) SetReference(ref *plumbing.Reference) error {
	_, err := r.r.Set(r.ctx, ref)
	return err
}

type objsStorer struct {
	ctx context.Context
	r   *ObjsRepo
}

func NewEncodedObjectStorer(ctx context.Context, r *ObjsRepo) storer.EncodedObjectStorer {
	return &objsStorer{ctx, r}
}

// AddAlternate implements [storer.EncodedObjectStorer].
func (o *objsStorer) AddAlternate(remote string) error {
	return errors.ErrUnsupported
}

// EncodedObject implements [storer.EncodedObjectStorer].
func (o *objsStorer) EncodedObject(t plumbing.ObjectType, h plumbing.Hash) (plumbing.EncodedObject, error) {
	return o.r.Get(o.ctx, t, h)
}

// EncodedObjectSize implements [storer.EncodedObjectStorer].
func (o *objsStorer) EncodedObjectSize(h plumbing.Hash) (int64, error) {
	obj, err := o.r.Get(o.ctx, plumbing.AnyObject, h)
	if err != nil {
		return 0, err
	}
	return obj.Size(), nil
}

// HasEncodedObject implements [storer.EncodedObjectStorer].
func (o *objsStorer) HasEncodedObject(h plumbing.Hash) error {
	exists, err := o.r.Has(o.ctx, plumbing.AnyObject, h)
	if err != nil {
		return err
	}
	if !exists {
		return plumbing.ErrObjectNotFound
	}
	return nil
}

// IterEncodedObjects implements [storer.EncodedObjectStorer].
func (o *objsStorer) IterEncodedObjects(plumbing.ObjectType) (storer.EncodedObjectIter, error) {
	return nil, errors.ErrUnsupported
}

// NewEncodedObject implements [storer.EncodedObjectStorer].
func (o *objsStorer) NewEncodedObject() plumbing.EncodedObject {
	return &plumbing.MemoryObject{}
}

// SetEncodedObject implements [storer.EncodedObjectStorer].
func (o *objsStorer) SetEncodedObject(obj plumbing.EncodedObject) (plumbing.Hash, error) {
	rd, err := obj.Reader()
	if err != nil {
		return plumbing.ZeroHash, err
	}

	blob, err := io.ReadAll(rd)
	if err != nil {
		return plumbing.ZeroHash, err
	}

	err = o.r.Put(o.ctx, obj.Type(), obj.Hash(), blob)
	return obj.Hash(), err
}
